//! NAND Reader Tab
//!
//! Provides a GUI for communicating with the NAND Controller over a serial
//! (FTDI/ttyUSB) interface.  The controller protocol is documented in
//! "NAND Controller-Documentation.txt".
//!
//! Key protocol commands used here:
//!   0x01  M_RESET              – controller reset
//!   0x04  M_NAND_RESET         – NAND reset
//!   0x06  M_NAND_READ_ID       – read JEDEC ID into internal buffer
//!   0x0D  MI_GET_STATUS        – get controller status register
//!   0x0E  MI_CHIP_ENABLE       – enable CE# for a given CE line
//!   0x0F  MI_CHIP_DISABLE      – disable CE# for a given CE line
//!   0x12  MI_RESET_INDEX       – reset internal buffer index
//!   0x13  MI_GET_ID_BYTE       – read one byte from JEDEC ID buffer
//!   0x1D  M_SET_PAGESIZE       – set page size in bytes
//!   0x09  M_NAND_READ          – read page into internal buffer
//!   0x15  MI_GET_DATA_PAGE_BYTE – read one byte from data page buffer
//!   0x18  MI_SET_CURRENT_ADDRESS_BYTE – set one address byte
//!
//! Serial framing (inferred from docs):
//!   Write: [CMD, DATA_IN]  (2 bytes; DATA_IN = 0x00 when not used)
//!   Read : after the 2-byte write the controller responds with [DATA_OUT]
//!          on commands that have a Data_out field.
//!
//! All serial I/O is performed on a background thread so the UI stays
//! responsive.  Commands are sent via an `mpsc::Sender<ReaderCommand>` and
//! responses (log lines, ID bytes, page data) are received via
//! `mpsc::Receiver<ReaderEvent>`.

use egui::{Color32, RichText, ScrollArea, TextEdit, Ui};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

// ── Protocol constants ────────────────────────────────────────────────────────

const CMD_M_RESET: u8 = 0x01;
const CMD_M_NAND_RESET: u8 = 0x04;
const CMD_M_NAND_READ_ID: u8 = 0x06;
const CMD_MI_GET_STATUS: u8 = 0x0D;
const CMD_MI_CHIP_ENABLE: u8 = 0x0E;
const CMD_MI_CHIP_DISABLE: u8 = 0x0F;
const CMD_MI_RESET_INDEX: u8 = 0x12;
const CMD_MI_GET_ID_BYTE: u8 = 0x13;
const CMD_M_NAND_READ: u8 = 0x09;
const CMD_MI_GET_DATA_PAGE_BYTE: u8 = 0x15;
const CMD_MI_SET_CURRENT_ADDRESS_BYTE: u8 = 0x18;
const CMD_M_SET_PAGESIZE: u8 = 0x1D;

/// Number of CE lines shown in the ID table.
const NUM_CE_LINES: usize = 8;

/// Bytes per JEDEC ID read (5 bytes per the NAND controller documentation).
const JEDEC_ID_BYTES: usize = 5;

/// Default baud rate for the FTDI UART link.
const DEFAULT_BAUD: u32 = 115_200;

// ── Inter-thread messaging ────────────────────────────────────────────────────

/// Commands sent from the UI thread to the serial worker thread.
#[derive(Debug)]
enum ReaderCommand {
    /// Scan all CE lines for JEDEC IDs.
    ScanIds,
    /// Read the full NAND starting from page 0 and write to `dump_path`.
    StartDump {
        dump_path: String,
        page_size: u32,
        pages_per_block: u32,
        num_blocks: u32,
    },
    /// Abort an in-progress dump.
    AbortDump,
    /// Disconnect and stop the worker thread.
    Disconnect,
}

/// Events sent from the serial worker thread back to the UI.
#[derive(Debug)]
enum ReaderEvent {
    /// Informational log line.
    Log(String),
    /// JEDEC ID result for one CE line (ce_index, [b0..b4]).
    JedecId { ce: usize, bytes: [u8; JEDEC_ID_BYTES] },
    /// Controller status byte.
    Status(u8),
    /// One page of dump data has been written; carries (pages_done, total_pages).
    DumpProgress { pages_done: u64, total_pages: u64 },
    /// Dump finished successfully.
    DumpComplete,
    /// Fatal error on the serial port.
    Error(String),
}

// ── Settings (persisted) ──────────────────────────────────────────────────────

/// Settings that are saved to disk across sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NandReaderSettings {
    /// Last-used serial device path, e.g. "/dev/ttyUSB0".
    pub device_path: String,
    /// Baud rate.
    pub baud_rate: u32,
    /// Page size in bytes (e.g. 2048, 4096).
    pub page_size: u32,
    /// Pages per block (e.g. 64, 128).
    pub pages_per_block: u32,
    /// Number of blocks.
    pub num_blocks: u32,
    /// Default dump file name.
    pub dump_filename: String,
}

impl Default for NandReaderSettings {
    fn default() -> Self {
        Self {
            device_path: String::new(),
            baud_rate: DEFAULT_BAUD,
            page_size: 4096,
            pages_per_block: 64,
            num_blocks: 1024,
            dump_filename: "01_01.dump".to_string(),
        }
    }
}

impl NandReaderSettings {
    fn state_path() -> PathBuf {
        let base = std::env::var("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::var("HOME")
                    .map(|h| PathBuf::from(h).join(".local").join("state"))
                    .unwrap_or_else(|_| PathBuf::from("."))
            });
        base.join("nand-flash-viewer").join("nand_reader.json")
    }

    pub fn load() -> Self {
        let p = Self::state_path();
        if let Ok(content) = std::fs::read_to_string(&p) {
            if let Ok(s) = serde_json::from_str(&content) {
                return s;
            }
        }
        Self::default()
    }

    pub fn save(&self) {
        let p = Self::state_path();
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&p, json);
        }
    }
}

// ── Connection status ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

// ── Dump status ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DumpStatus {
    Idle,
    Running,
    Done,
    Aborted,
    Error(String),
}

// ── JEDEC ID entry ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct JedecIdEntry {
    pub bytes: Option<[u8; JEDEC_ID_BYTES]>,
    /// Human-readable decode: manufacturer name from byte 0.
    pub manufacturer: &'static str,
}

impl Default for JedecIdEntry {
    fn default() -> Self {
        Self {
            bytes: None,
            manufacturer: "",
        }
    }
}

/// Decode the JEDEC manufacturer ID byte into a name string.
fn decode_manufacturer(id: u8) -> &'static str {
    match id {
        0x2C => "Micron",
        0xEC => "Samsung",
        0x98 => "Toshiba/Kioxia",
        0x45 => "SanDisk",
        0x01 => "AMD/Spansion",
        0x04 => "Fujitsu",
        0x07 => "Renesas",
        0x20 => "ST Micro / Numonyx",
        0xAD => "SK Hynix",
        0xC8 => "GigaDevice",
        0xC2 => "Macronix",
        0x9E => "ATO Solution",
        0xEF => "Winbond",
        0x00 | 0xFF => "(no chip)",
        _ => "Unknown",
    }
}

// ── Tab state ─────────────────────────────────────────────────────────────────

/// Persistent + runtime state for the NAND Reader tab.
pub struct NandReaderTabState {
    // ── settings ──────────────────────────────────────────────────────────
    pub settings: NandReaderSettings,

    // ── device list cache ─────────────────────────────────────────────────
    /// Available serial ports, refreshed on first render and on demand.
    port_list: Vec<PortEntry>,
    /// Whether the port list has been populated at least once.
    port_list_loaded: bool,

    // ── connection ─────────────────────────────────────────────────────────
    pub connection_status: ConnectionStatus,
    /// Vendor/device info string fetched from the OS serial port info.
    pub device_info: String,
    /// Controller firmware status byte (from MI_GET_STATUS after connect).
    pub controller_status: Option<u8>,

    // ── worker thread channels ─────────────────────────────────────────────
    cmd_tx: Option<mpsc::Sender<ReaderCommand>>,
    evt_rx: Option<mpsc::Receiver<ReaderEvent>>,

    // ── JEDEC ID table ────────────────────────────────────────────────────
    pub jedec_ids: [JedecIdEntry; NUM_CE_LINES],
    pub scan_done: bool,

    // ── dump ──────────────────────────────────────────────────────────────
    pub dump_status: DumpStatus,
    pub dump_pages_done: u64,
    pub dump_total_pages: u64,

    // ── log ───────────────────────────────────────────────────────────────
    pub log_lines: Vec<String>,
    pub auto_scroll_log: bool,

    // ── settings_dirty flag ───────────────────────────────────────────────
    settings_dirty: bool,
}

/// A discovered serial port entry — USB ports only.
#[derive(Debug, Clone)]
struct PortEntry {
    /// e.g. "/dev/ttyUSB0"
    path: String,
    /// Short combo label: "  VID:PID  Manufacturer Product"
    combo_label: String,
    /// Full detail string shown beside the combo when this port is selected.
    detail: String,
}

impl Default for NandReaderTabState {
    fn default() -> Self {
        Self::new()
    }
}

impl NandReaderTabState {
    pub fn new() -> Self {
        let settings = NandReaderSettings::load();
        Self {
            settings,
            port_list: Vec::new(),
            port_list_loaded: false,
            connection_status: ConnectionStatus::Disconnected,
            device_info: String::new(),
            controller_status: None,
            cmd_tx: None,
            evt_rx: None,
            jedec_ids: std::array::from_fn(|_| JedecIdEntry::default()),
            scan_done: false,
            dump_status: DumpStatus::Idle,
            dump_pages_done: 0,
            dump_total_pages: 0,
            log_lines: Vec::new(),
            auto_scroll_log: true,
            settings_dirty: false,
        }
    }

    // ── port discovery ────────────────────────────────────────────────────

    fn refresh_port_list(&mut self) {
        self.port_list.clear();
        match serialport::available_ports() {
            Ok(ports) => {
                for p in ports {
                    // Only show USB-backed ports; skip /dev/ttyS*, Bluetooth, PCI, Unknown.
                    if let serialport::SerialPortType::UsbPort(usb) = &p.port_type {
                        let mfr  = usb.manufacturer.as_deref().unwrap_or("").trim().to_string();
                        let prod = usb.product.as_deref().unwrap_or("").trim().to_string();
                        let sn   = usb.serial_number.as_deref().unwrap_or("").trim().to_string();

                        // Short label that fits inside the combo box item row:
                        // "/dev/ttyUSB0   VID:0403 PID:6001   FTDI FT232R"
                        let mut combo_label = format!(
                            "{}   VID:{:04X} PID:{:04X}",
                            p.port_name, usb.vid, usb.pid
                        );
                        // Append manufacturer / product if available
                        if !mfr.is_empty() || !prod.is_empty() {
                            combo_label.push_str("   ");
                            if !mfr.is_empty() {
                                combo_label.push_str(&mfr);
                                combo_label.push(' ');
                            }
                            if !prod.is_empty() {
                                combo_label.push_str(&prod);
                            }
                        }

                        // Detail line shown beside the combo when this port is active:
                        // "VID:0403 PID:6001 — FTDI FT232R USB UART  [SN: A12345]"
                        let mut detail = format!(
                            "VID:{:04X} PID:{:04X}",
                            usb.vid, usb.pid
                        );
                        if !mfr.is_empty() || !prod.is_empty() {
                            detail.push_str(" — ");
                            if !mfr.is_empty() {
                                detail.push_str(&mfr);
                                detail.push(' ');
                            }
                            if !prod.is_empty() {
                                detail.push_str(&prod);
                            }
                        }
                        if !sn.is_empty() {
                            detail.push_str(&format!("  [SN: {}]", sn));
                        }

                        self.port_list.push(PortEntry {
                            path: p.port_name,
                            combo_label,
                            detail,
                        });
                    }
                    // Non-USB ports (ttyS*, Bluetooth, PCI, Unknown) are intentionally skipped.
                }
            }
            Err(e) => {
                self.log(format!("Port discovery error: {}", e));
            }
        }
        self.port_list_loaded = true;
    }

    fn device_info_for_path(&self, path: &str) -> String {
        for entry in &self.port_list {
            if entry.path == path {
                return entry.detail.clone();
            }
        }
        String::new()
    }

    // ── logging ───────────────────────────────────────────────────────────

    fn log(&mut self, msg: impl Into<String>) {
        self.log_lines.push(msg.into());
        // Keep log bounded
        if self.log_lines.len() > 2000 {
            self.log_lines.drain(0..500);
        }
    }

    // ── connection ────────────────────────────────────────────────────────

    /// Spawn the serial worker thread and connect to the device.
    pub fn connect(&mut self) {
        if self.settings.device_path.trim().is_empty() {
            self.log("No device selected.");
            return;
        }
        if self.connection_status == ConnectionStatus::Connected
            || self.connection_status == ConnectionStatus::Connecting
        {
            return;
        }

        self.connection_status = ConnectionStatus::Connecting;
        self.device_info = self.device_info_for_path(&self.settings.device_path);
        self.log(format!(
            "Connecting to {} at {} baud…",
            self.settings.device_path, self.settings.baud_rate
        ));

        let (cmd_tx, cmd_rx) = mpsc::channel::<ReaderCommand>();
        let (evt_tx, evt_rx) = mpsc::channel::<ReaderEvent>();

        let device_path = self.settings.device_path.clone();
        let baud = self.settings.baud_rate;

        thread::spawn(move || {
            serial_worker(device_path, baud, cmd_rx, evt_tx);
        });

        self.cmd_tx = Some(cmd_tx);
        self.evt_rx = Some(evt_rx);
    }

    /// Send a disconnect command to the worker thread.
    pub fn disconnect(&mut self) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(ReaderCommand::Disconnect);
        }
        self.cmd_tx = None;
        self.evt_rx = None;
        self.connection_status = ConnectionStatus::Disconnected;
        self.controller_status = None;
        self.log("Disconnected.");
    }

    /// Drain all pending events from the worker thread and update state.
    fn poll_events(&mut self) {
        // Collect events first to avoid holding borrow across self.log() calls
        let mut events = Vec::new();
        if let Some(rx) = &self.evt_rx {
            while let Ok(ev) = rx.try_recv() {
                events.push(ev);
            }
        }
        for ev in events {
            match ev {
                ReaderEvent::Log(msg) => {
                    self.log(msg);
                }
                ReaderEvent::Status(s) => {
                    self.controller_status = Some(s);
                    self.connection_status = ConnectionStatus::Connected;
                    self.log(format!("Connected. Controller status: 0x{:02X}", s));
                }
                ReaderEvent::JedecId { ce, bytes } => {
                    let mfr = decode_manufacturer(bytes[0]);
                    self.log(format!(
                        "CE#{}: {:02X} {:02X} {:02X} {:02X} {:02X}  ({})",
                        ce,
                        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4],
                        mfr
                    ));
                    if ce < NUM_CE_LINES {
                        self.jedec_ids[ce] = JedecIdEntry {
                            bytes: Some(bytes),
                            manufacturer: mfr,
                        };
                    }
                    // Mark scan done once the last CE is reported
                    if ce + 1 == NUM_CE_LINES {
                        self.scan_done = true;
                    }
                }
                ReaderEvent::DumpProgress { pages_done, total_pages } => {
                    self.dump_pages_done = pages_done;
                    self.dump_total_pages = total_pages;
                    self.dump_status = DumpStatus::Running;
                }
                ReaderEvent::DumpComplete => {
                    self.dump_status = DumpStatus::Done;
                    self.log("Dump complete.");
                }
                ReaderEvent::Error(e) => {
                    self.connection_status = ConnectionStatus::Error(e.clone());
                    self.dump_status = DumpStatus::Error(e.clone());
                    self.log(format!("ERROR: {}", e));
                    // Channel is dead; clean up
                    self.cmd_tx = None;
                    self.evt_rx = None;
                }
            }
        }
    }

    // ── commands ──────────────────────────────────────────────────────────

    fn send_cmd(&mut self, cmd: ReaderCommand) {
        if let Some(tx) = &self.cmd_tx {
            if tx.send(cmd).is_err() {
                self.log("Worker thread disconnected unexpectedly.");
                self.connection_status = ConnectionStatus::Error("Worker died".to_string());
                self.cmd_tx = None;
                self.evt_rx = None;
            }
        }
    }

    pub fn scan_ids(&mut self) {
        self.scan_done = false;
        // Clear previous results
        for e in &mut self.jedec_ids {
            *e = JedecIdEntry::default();
        }
        self.log("Scanning JEDEC IDs on CE#0..CE#7…");
        self.send_cmd(ReaderCommand::ScanIds);
    }

    pub fn start_dump(&mut self) {
        self.dump_status = DumpStatus::Running;
        self.dump_pages_done = 0;
        self.dump_total_pages =
            self.settings.num_blocks as u64 * self.settings.pages_per_block as u64;
        let dump_path = self.settings.dump_filename.clone();
        self.log(format!("Starting dump → {}", dump_path));
        self.send_cmd(ReaderCommand::StartDump {
            dump_path,
            page_size: self.settings.page_size,
            pages_per_block: self.settings.pages_per_block,
            num_blocks: self.settings.num_blocks,
        });
    }

    pub fn abort_dump(&mut self) {
        self.log("Aborting dump…");
        self.send_cmd(ReaderCommand::AbortDump);
        self.dump_status = DumpStatus::Aborted;
    }

    // ── UI ────────────────────────────────────────────────────────────────

    pub fn show_ui(&mut self, ctx: &egui::Context) {
        // Drain worker events every frame
        self.poll_events();

        // Save settings if they changed (debounced – only on focus-lost etc.)
        if self.settings_dirty {
            self.settings.save();
            self.settings_dirty = false;
        }

        // Lazy port-list refresh
        if !self.port_list_loaded {
            self.refresh_port_list();
        }

        egui::TopBottomPanel::top("nand_reader_top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("NAND Reader");
                ui.separator();
                self.render_connection_bar(ui);
            });
        });

        egui::TopBottomPanel::bottom("nand_reader_bottom")
            .min_height(120.0)
            .show(ctx, |ui| {
                self.render_log(ui);
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.render_id_table(ui);
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                self.render_settings(ui);
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                self.render_dump_controls(ui);
            });
        });
    }

    // ── connection bar ────────────────────────────────────────────────────

    fn render_connection_bar(&mut self, ui: &mut Ui) {
        // Refresh button
        if ui.small_button("🔄").on_hover_text("Refresh port list").clicked() {
            self.refresh_port_list();
        }

        // Port combo — show full "path   VID:PID   Mfr Product" for each USB entry.
        // The selected-text (collapsed state) also shows the full combo_label so
        // the user can still see which device is active without opening the dropdown.
        let selected_label = self.port_list.iter()
            .find(|e| e.path == self.settings.device_path)
            .map(|e| e.combo_label.clone())
            .unwrap_or_else(|| {
                if self.settings.device_path.is_empty() {
                    "— select device —".to_string()
                } else {
                    // Previously saved path no longer in list; show path alone.
                    self.settings.device_path.clone()
                }
            });

        egui::ComboBox::from_id_source("nand_port_select")
            .selected_text(&selected_label)
            .width(420.0)
            .show_ui(ui, |ui| {
                for entry in &self.port_list.clone() {
                    if ui.selectable_value(
                        &mut self.settings.device_path,
                        entry.path.clone(),
                        &entry.combo_label,
                    ).clicked() {
                        self.device_info = entry.detail.clone();
                        self.settings_dirty = true;
                    }
                }
                if self.port_list.is_empty() {
                    ui.colored_label(
                        egui::Color32::GRAY,
                        "No USB serial ports detected — click 🔄 to refresh",
                    );
                }
            });

        // The full USB info is already shown in the combo selected text; no
        // separate label is needed. We still populate self.device_info lazily
        // so connect() can log it.
        if self.device_info.is_empty() && !self.settings.device_path.is_empty() {
            let info = self.device_info_for_path(&self.settings.device_path);
            if !info.is_empty() {
                self.device_info = info;
            }
        }

        ui.separator();

        // Baud rate selector
        ui.label("Baud:");
        let mut baud_str = self.settings.baud_rate.to_string();
        let baud_edit = ui.add(
            TextEdit::singleline(&mut baud_str)
                .desired_width(70.0)
                .hint_text("115200"),
        );
        if baud_edit.changed() {
            if let Ok(b) = baud_str.parse::<u32>() {
                self.settings.baud_rate = b;
                self.settings_dirty = true;
            }
        }

        ui.separator();

        // Connect / Disconnect button
        match &self.connection_status {
            ConnectionStatus::Disconnected | ConnectionStatus::Error(_) => {
                let enabled = !self.settings.device_path.trim().is_empty();
                if ui
                    .add_enabled(enabled, egui::Button::new("Connect"))
                    .on_hover_text("Open serial port and verify controller protocol")
                    .clicked()
                {
                    self.connect();
                }
            }
            ConnectionStatus::Connecting => {
                ui.add_enabled(false, egui::Button::new("Connecting…"));
            }
            ConnectionStatus::Connected => {
                if ui.button("Disconnect").clicked() {
                    self.disconnect();
                }
            }
        }

        // Status indicator
        let (dot, color) = match &self.connection_status {
            ConnectionStatus::Disconnected => ("●", Color32::GRAY),
            ConnectionStatus::Connecting => ("●", Color32::YELLOW),
            ConnectionStatus::Connected => ("●", Color32::GREEN),
            ConnectionStatus::Error(_) => ("●", Color32::RED),
        };
        ui.colored_label(color, dot);

        if let ConnectionStatus::Error(e) = &self.connection_status.clone() {
            ui.colored_label(Color32::RED, e);
        } else if let Some(s) = self.controller_status {
            ui.label(RichText::new(format!("Status: 0x{:02X}", s)).weak().small());
        }
    }

    // ── JEDEC ID table ────────────────────────────────────────────────────

    fn render_id_table(&mut self, ui: &mut Ui) {
        let connected = self.connection_status == ConnectionStatus::Connected;

        ui.horizontal(|ui| {
            ui.strong("JEDEC IDs  (M_NAND_READ_ID)");
            ui.add_space(12.0);
            if ui
                .add_enabled(connected, egui::Button::new("🔍 Scan Chips"))
                .on_hover_text("Enable each CE# in turn, read JEDEC ID, disable CE#")
                .clicked()
            {
                self.scan_ids();
            }
            if self.scan_done {
                ui.colored_label(Color32::GREEN, "✔ done");
            }
        });

        ui.add_space(4.0);

        // Table header
        let row_height = 20.0;
        egui::Grid::new("jedec_id_table")
            .num_columns(8)
            .spacing([6.0, 2.0])
            .striped(true)
            .show(ui, |ui| {
                // Header row
                ui.strong("CE#");
                ui.strong("Byte 0\n(MFR)");
                ui.strong("Byte 1");
                ui.strong("Byte 2");
                ui.strong("Byte 3");
                ui.strong("Byte 4");
                ui.strong("Manufacturer");
                ui.strong("Status");
                ui.end_row();

                for (ce, entry) in self.jedec_ids.iter().enumerate() {
                    ui.label(format!("CE#{}", ce));
                    match entry.bytes {
                        None => {
                            for _ in 0..5 {
                                ui.colored_label(Color32::DARK_GRAY, "--");
                            }
                            ui.colored_label(Color32::DARK_GRAY, "—");
                            ui.colored_label(Color32::DARK_GRAY, "not scanned");
                        }
                        Some(bytes) => {
                            let is_empty =
                                bytes[0] == 0x00 || bytes[0] == 0xFF;
                            let id_color = if is_empty {
                                Color32::DARK_GRAY
                            } else {
                                Color32::WHITE
                            };
                            for b in &bytes {
                                ui.colored_label(
                                    id_color,
                                    format!("{:02X}", b),
                                );
                            }
                            let mfr_color = if is_empty {
                                Color32::DARK_GRAY
                            } else {
                                Color32::from_rgb(100, 230, 140)
                            };
                            ui.colored_label(mfr_color, entry.manufacturer);
                            if is_empty {
                                ui.colored_label(Color32::DARK_GRAY, "no chip");
                            } else {
                                ui.colored_label(Color32::GREEN, "✔ chip found");
                            }
                        }
                    }
                    let _ = row_height; // used by egui grid implicitly
                    ui.end_row();
                }
            });
    }

    // ── settings ──────────────────────────────────────────────────────────

    fn render_settings(&mut self, ui: &mut Ui) {
        ui.strong("NAND Controller Settings");
        ui.add_space(4.0);

        egui::Grid::new("nand_settings_grid")
            .num_columns(4)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                // Page size
                ui.label("Page size (bytes):");
                let ps_drag = ui.add(
                    egui::DragValue::new(&mut self.settings.page_size)
                        .clamp_range(256u32..=65536u32)
                        .speed(256.0),
                );
                if ps_drag.changed() {
                    self.settings_dirty = true;
                }

                // Pages per block
                ui.label("Pages / block:");
                let ppb_drag = ui.add(
                    egui::DragValue::new(&mut self.settings.pages_per_block)
                        .clamp_range(1u32..=512u32)
                        .speed(8.0),
                );
                if ppb_drag.changed() {
                    self.settings_dirty = true;
                }
                ui.end_row();

                // Number of blocks
                ui.label("Number of blocks:");
                let nb_drag = ui.add(
                    egui::DragValue::new(&mut self.settings.num_blocks)
                        .clamp_range(1u32..=1_048_576u32)
                        .speed(64.0),
                );
                if nb_drag.changed() {
                    self.settings_dirty = true;
                }

                // Computed total size
                let total_bytes = self.settings.page_size as u64
                    * self.settings.pages_per_block as u64
                    * self.settings.num_blocks as u64;
                let total_mb = total_bytes as f64 / (1024.0 * 1024.0);
                ui.label(
                    RichText::new(format!("= {:.1} MB  ({} bytes total)", total_mb, total_bytes))
                        .weak()
                        .small(),
                );
                ui.end_row();
            });
    }

    // ── dump controls ─────────────────────────────────────────────────────

    fn render_dump_controls(&mut self, ui: &mut Ui) {
        let connected = self.connection_status == ConnectionStatus::Connected;

        ui.strong("Dump to File");
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.label("Dump file:");
            let fn_edit = ui.add(
                TextEdit::singleline(&mut self.settings.dump_filename)
                    .desired_width(240.0)
                    .hint_text("01_01.dump"),
            );
            if fn_edit.changed() {
                self.settings_dirty = true;
            }
        });

        ui.add_space(4.0);

        match &self.dump_status {
            DumpStatus::Idle | DumpStatus::Done | DumpStatus::Aborted | DumpStatus::Error(_) => {
                let label = match &self.dump_status {
                    DumpStatus::Done => "▶ Dump Again",
                    DumpStatus::Aborted => "▶ Retry Dump",
                    DumpStatus::Error(_) => "▶ Retry Dump",
                    _ => "▶ Start Dump",
                };
                if ui
                    .add_enabled(connected, egui::Button::new(label))
                    .on_hover_text("Read all NAND pages and write to dump file")
                    .clicked()
                {
                    self.start_dump();
                }
                if let DumpStatus::Done = &self.dump_status {
                    ui.colored_label(Color32::GREEN, "✔ Dump complete");
                }
                if let DumpStatus::Aborted = &self.dump_status {
                    ui.colored_label(Color32::YELLOW, "⚠ Aborted");
                }
                if let DumpStatus::Error(e) = &self.dump_status.clone() {
                    ui.colored_label(Color32::RED, format!("✗ {}", e));
                }
            }
            DumpStatus::Running => {
                let total = self.dump_total_pages.max(1);
                let ratio = (self.dump_pages_done as f32 / total as f32).clamp(0.0, 1.0);
                let done_mb = (self.dump_pages_done * self.settings.page_size as u64) as f64
                    / (1024.0 * 1024.0);
                let total_mb = (total * self.settings.page_size as u64) as f64 / (1024.0 * 1024.0);
                ui.add(
                    egui::ProgressBar::new(ratio).text(format!(
                        "{:.1}%  ({:.1} / {:.1} MB)",
                        ratio * 100.0,
                        done_mb,
                        total_mb
                    )),
                );
                if ui.button("⏹ Abort").clicked() {
                    self.abort_dump();
                }
            }
        }
    }

    // ── log ───────────────────────────────────────────────────────────────

    fn render_log(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.strong("Log");
            ui.checkbox(&mut self.auto_scroll_log, "Auto-scroll");
            if ui.small_button("🗑 Clear").clicked() {
                self.log_lines.clear();
            }
        });

        ScrollArea::vertical()
            .id_source("nand_log_scroll")
            .auto_shrink([false; 2])
            .stick_to_bottom(self.auto_scroll_log)
            .show(ui, |ui| {
                for line in &self.log_lines {
                    let color = if line.starts_with("ERROR") || line.starts_with("✗") {
                        Color32::RED
                    } else if line.starts_with("Connected") || line.starts_with("✔") {
                        Color32::GREEN
                    } else if line.starts_with("WARNING") || line.starts_with("⚠") {
                        Color32::YELLOW
                    } else {
                        Color32::LIGHT_GRAY
                    };
                    ui.colored_label(color, line);
                }
            });
    }
}

// ── Serial worker thread ──────────────────────────────────────────────────────

/// All serial I/O happens here, isolated from the UI thread.
fn serial_worker(
    device_path: String,
    baud: u32,
    cmd_rx: mpsc::Receiver<ReaderCommand>,
    evt_tx: mpsc::Sender<ReaderEvent>,
) {
    // Rename for macro hygiene
    let evt_tx_ref = &evt_tx;
    macro_rules! log {
        ($($arg:tt)*) => {
            let _ = evt_tx_ref.send(ReaderEvent::Log(format!($($arg)*)));
        };
    }

    // Open port
    let port_result = serialport::new(&device_path, baud)
        .timeout(Duration::from_millis(500))
        .open();

    let mut port = match port_result {
        Ok(p) => {
            log!("Serial port opened: {} @ {} baud", device_path, baud);
            p
        }
        Err(e) => {
            let _ = evt_tx.send(ReaderEvent::Error(format!("Cannot open {}: {}", device_path, e)));
            return;
        }
    };

    // ── Low-level helpers ─────────────────────────────────────────────────

    /// Send a 2-byte command frame [CMD, DATA_IN] and optionally read
    /// 1 byte of response if `expect_response` is true.
    fn write_cmd(
        port: &mut dyn serialport::SerialPort,
        cmd: u8,
        data_in: u8,
        expect_response: bool,
    ) -> Result<Option<u8>, String> {
        port.write_all(&[cmd, data_in])
            .map_err(|e| format!("write error: {}", e))?;
        port.flush().map_err(|e| format!("flush error: {}", e))?;

        if expect_response {
            let mut buf = [0u8; 1];
            // Retry up to 20 ms to handle slow controller responses
            let deadline = std::time::Instant::now() + Duration::from_millis(200);
            loop {
                match port.read(&mut buf) {
                    Ok(1) => return Ok(Some(buf[0])),
                    Ok(_) => {}
                    Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(e) => return Err(format!("read error: {}", e)),
                }
                if std::time::Instant::now() > deadline {
                    return Err("Timeout waiting for response".to_string());
                }
            }
        } else {
            Ok(None)
        }
    }

    // ── Reset + status probe ──────────────────────────────────────────────

    // Perform a controller reset and read status to confirm the connection
    match write_cmd(&mut *port, CMD_M_RESET, 0x00, false) {
        Err(e) => {
            let _ = evt_tx.send(ReaderEvent::Error(format!("Reset failed: {}", e)));
            return;
        }
        Ok(_) => {}
    }
    // Small delay to let controller process reset
    thread::sleep(Duration::from_millis(50));

    match write_cmd(&mut *port, CMD_MI_GET_STATUS, 0x00, true) {
        Ok(Some(s)) => {
            let _ = evt_tx.send(ReaderEvent::Status(s));
        }
        Ok(None) => {
            let _ = evt_tx.send(ReaderEvent::Error("No status response from controller".to_string()));
            return;
        }
        Err(e) => {
            let _ = evt_tx.send(ReaderEvent::Error(format!("Status probe failed: {}", e)));
            return;
        }
    }

    // ── Main command loop ─────────────────────────────────────────────────

    'main: loop {
        let cmd = match cmd_rx.recv() {
            Ok(c) => c,
            Err(_) => break, // channel closed
        };

        match cmd {
            ReaderCommand::Disconnect => break 'main,

            ReaderCommand::ScanIds => {
                for ce in 0..NUM_CE_LINES {
                    // Enable chip
                    if let Err(e) =
                        write_cmd(&mut *port, CMD_MI_CHIP_ENABLE, ce as u8, false)
                    {
                        log!("CE#{} enable error: {}", ce, e);
                        continue;
                    }
                    thread::sleep(Duration::from_millis(5));

                    // NAND reset
                    if let Err(e) = write_cmd(&mut *port, CMD_M_NAND_RESET, 0x00, false) {
                        log!("CE#{} NAND reset error: {}", ce, e);
                        let _ = write_cmd(&mut *port, CMD_MI_CHIP_DISABLE, ce as u8, false);
                        continue;
                    }
                    thread::sleep(Duration::from_millis(5));

                    // Read ID
                    if let Err(e) = write_cmd(&mut *port, CMD_M_NAND_READ_ID, 0x00, false) {
                        log!("CE#{} READ_ID error: {}", ce, e);
                        let _ = write_cmd(&mut *port, CMD_MI_CHIP_DISABLE, ce as u8, false);
                        continue;
                    }
                    thread::sleep(Duration::from_millis(5));

                    // Reset index
                    if let Err(e) = write_cmd(&mut *port, CMD_MI_RESET_INDEX, 0x00, false) {
                        log!("CE#{} RESET_INDEX error: {}", ce, e);
                        let _ = write_cmd(&mut *port, CMD_MI_CHIP_DISABLE, ce as u8, false);
                        continue;
                    }

                    // Read 5 ID bytes
                    let mut id_bytes = [0u8; JEDEC_ID_BYTES];
                    let mut ok = true;
                    for b in &mut id_bytes {
                        match write_cmd(&mut *port, CMD_MI_GET_ID_BYTE, 0x00, true) {
                            Ok(Some(byte)) => *b = byte,
                            Ok(None) => {
                                log!("CE#{} GET_ID_BYTE: no response", ce);
                                ok = false;
                                break;
                            }
                            Err(e) => {
                                log!("CE#{} GET_ID_BYTE error: {}", ce, e);
                                ok = false;
                                break;
                            }
                        }
                    }

                    // Disable chip
                    let _ = write_cmd(&mut *port, CMD_MI_CHIP_DISABLE, ce as u8, false);

                    if ok {
                        let _ = evt_tx.send(ReaderEvent::JedecId { ce, bytes: id_bytes });
                    }

                    // Check for abort between CE lines
                    if let Ok(pending) = cmd_rx.try_recv() {
                        match pending {
                            ReaderCommand::Disconnect => break 'main,
                            ReaderCommand::AbortDump => {}
                            other => {
                                // Re-queue if it is something else (best effort)
                                let _ = evt_tx.send(ReaderEvent::Log(
                                    format!("Unexpected command during scan: {:?}", other)
                                ));
                            }
                        }
                    }
                }
            }

            ReaderCommand::StartDump { dump_path, page_size, pages_per_block, num_blocks } => {
                let total_pages = pages_per_block as u64 * num_blocks as u64;

                let mut file = match std::fs::File::create(&dump_path) {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = evt_tx.send(ReaderEvent::Error(
                            format!("Cannot create dump file '{}': {}", dump_path, e),
                        ));
                        continue;
                    }
                };

                // Set page size on controller
                // The protocol sends size in bytes; for sizes > 255 we send the
                // high byte first then low byte via two writes using address bytes,
                // but the simple command only takes 1 DATA_IN byte.
                // For now send the low byte (works for ≤255 byte pages) and log a
                // warning for larger pages.  A full implementation would use the
                // address buffer mechanism.
                let ps_cmd_byte = if page_size <= 255 {
                    page_size as u8
                } else {
                    log!("Warning: page_size {} > 255; using address-buffer method not yet implemented, sending 0", page_size);
                    0u8
                };
                let _ = write_cmd(&mut *port, CMD_M_SET_PAGESIZE, ps_cmd_byte, false);
                thread::sleep(Duration::from_millis(5));

                let mut aborted = false;
                let mut page_global: u64 = 0;

                'blocks: for block in 0..num_blocks as u64 {
                    for page_in_block in 0..pages_per_block as u64 {
                        // Check for abort / disconnect
                        if let Ok(pending) = cmd_rx.try_recv() {
                            match pending {
                                ReaderCommand::AbortDump | ReaderCommand::Disconnect => {
                                    aborted = true;
                                    break 'blocks;
                                }
                                _ => {}
                            }
                        }

                        // Build 5-byte page address
                        // Row address = block * pages_per_block + page_in_block
                        // Column address = 0  (we read from byte 0)
                        // Byte layout (standard NAND, 5 address cycles):
                        //   col[7:0], col[11:8], row[7:0], row[15:8], row[23:16]
                        let row = block * pages_per_block as u64 + page_in_block;
                        let addr: [u8; 5] = [
                            0x00,                    // col low
                            0x00,                    // col high
                            (row & 0xFF) as u8,
                            ((row >> 8) & 0xFF) as u8,
                            ((row >> 16) & 0xFF) as u8,
                        ];

                        // Reset index then set address bytes
                        let _ = write_cmd(&mut *port, CMD_MI_RESET_INDEX, 0x00, false);
                        for &ab in &addr {
                            if let Err(e) = write_cmd(
                                &mut *port, CMD_MI_SET_CURRENT_ADDRESS_BYTE, ab, false,
                            ) {
                                log!("Set address byte error at page {}: {}", page_global, e);
                            }
                        }

                        // Issue page read
                        if let Err(e) = write_cmd(&mut *port, CMD_M_NAND_READ, 0x00, false) {
                            log!("READ page {} error: {}", page_global, e);
                            // Write zeroes for this page so the dump stays contiguous
                            let _ = file.write_all(&vec![0u8; page_size as usize]);
                            page_global += 1;
                            continue;
                        }

                        // Small delay for NAND read cycle
                        thread::sleep(Duration::from_micros(200));

                        // Read page bytes from the controller buffer
                        let _ = write_cmd(&mut *port, CMD_MI_RESET_INDEX, 0x00, false);
                        let mut page_buf = vec![0u8; page_size as usize];
                        let mut read_ok = true;
                        for b in &mut page_buf {
                            match write_cmd(&mut *port, CMD_MI_GET_DATA_PAGE_BYTE, 0x00, true) {
                                Ok(Some(byte)) => *b = byte,
                                Ok(None) | Err(_) => {
                                    read_ok = false;
                                    break;
                                }
                            }
                        }

                        if !read_ok {
                            log!("Read error at page {}; writing zeroes", page_global);
                            let _ = file.write_all(&vec![0u8; page_size as usize]);
                        } else if let Err(e) = file.write_all(&page_buf) {
                            let _ = evt_tx.send(ReaderEvent::Error(
                                format!("File write error: {}", e),
                            ));
                            aborted = true;
                            break 'blocks;
                        }

                        page_global += 1;

                        // Progress update every 16 pages
                        if page_global % 16 == 0 || page_global == total_pages {
                            let _ = evt_tx.send(ReaderEvent::DumpProgress {
                                pages_done: page_global,
                                total_pages,
                            });
                        }
                    }
                }

                if !aborted {
                    let _ = file.flush();
                    let _ = evt_tx.send(ReaderEvent::DumpComplete);
                }
            }

            ReaderCommand::AbortDump => {
                // No active dump; nothing to do
                log!("Abort received (no active dump).");
            }
        }
    }

    log!("Serial worker terminated.");
}
