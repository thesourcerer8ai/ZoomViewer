//! Main application window with viewport rendering and event handling

use crate::{
    workflow::WorkflowEditorState,
    AddressDisplay, CacheManager, DumpDataProvider, FileMetadata, HexTabState, PanController,
    PageStructureTabState, SearchTabState, TaskQueue, TileCoord, Viewport, ViewportManager,
    ViewportRenderer, ZoomController,
};
use fltk::{
    app::MouseWheel,
    enums::{ColorDepth, Event},
    frame::Frame,
    group::{Group, Tabs},
    image::RgbImage,
    prelude::*,
    text::TextEditor,
    window::{GlWindow, Window},
};
use egui_glow::Painter;
use fltk_egui::EguiState;
use image::RgbaImage;
use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Instant;

const TILE_WIDTH: u32 = 256;
const TILE_HEIGHT: u32 = 256;

/// Main application window
#[allow(dead_code)]
pub struct AppWindow {
    /// FLTK window
    window: Window,
    /// Workflow state
    workflow_state: Arc<Mutex<WorkflowEditorState>>,
    /// Search tab state
    pub search_tab_state: Arc<Mutex<SearchTabState>>,
    /// Hex tab state
    pub hex_tab_state: Arc<Mutex<HexTabState>>,
    /// Page structure editor tab state (shared with hex tab and workflow)
    pub page_structure_tab_state: Arc<Mutex<PageStructureTabState>>,
    /// Frame to display the viewport image
    viewport_frame: Arc<Mutex<Frame>>,
    /// Viewport manager for tile identification
    viewport_manager: Arc<Mutex<ViewportManager>>,
    /// Zoom controller
    zoom_controller: Arc<Mutex<ZoomController>>,
    /// Pan controller
    pan_controller: Arc<Mutex<PanController>>,
    /// Address display for mouse tracking
    address_display: Arc<Mutex<AddressDisplay>>,
    /// File metadata
    metadata: FileMetadata,
    /// Task queue for tile requests
    task_queue: Arc<TaskQueue>,
    /// Cache manager
    cache: Arc<CacheManager>,
    /// Viewport renderer for compositing tiles
    viewport_renderer: Arc<ViewportRenderer>,
    /// Current viewport image
    viewport_image: Arc<Mutex<RgbaImage>>,
    /// Cache of decoded FLTK tile images
    fltk_tile_cache: Arc<Mutex<HashMap<TileCoord, Arc<RgbImage>>>>,
    /// Mouse position (screen coordinates)
    mouse_x: i32,
    /// Mouse position (screen coordinates)
    mouse_y: i32,
    /// Status bar for address display
    status_bar: Arc<Mutex<TextEditor>>,
    /// Last render time
    last_render_time: Arc<Mutex<Option<Instant>>>,
    /// Last render duration
    last_render_duration: Arc<Mutex<f64>>,
}

/// Build egui FontDefinitions with ChakraPetch as the primary proportional
/// font, keeping egui's built-in font as fallback for icons and symbols.
fn chakra_font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "ChakraPetch".to_owned(),
        egui::FontData::from_static(include_bytes!("../assets/ChakraPetchMono-Medium.otf")),
    );
    // Prepend to both proportional and monospace families so it takes priority,
    // but the default egui font (which has full Unicode/icon coverage) remains
    // as fallback for any glyphs ChakraPetch doesn't cover.
    fonts.families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "ChakraPetch".to_owned());
    fonts.families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, "ChakraPetch".to_owned());
    fonts
}

impl AppWindow {

    /// Create a new application window
    pub fn new(
        metadata: FileMetadata,
        task_queue: Arc<TaskQueue>,
        cache: Arc<CacheManager>,
        tile_rx: Option<Receiver<TileCoord>>,
        file_loader: Option<Arc<parking_lot::Mutex<dyn DumpDataProvider>>>,
        initial_workflow: Option<WorkflowEditorState>,
    ) -> Self {
        // Extract filename from path for window title
        let filename = std::path::Path::new(&metadata.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unknown")
            .to_string();
        
        let mut window = Window::default()
            .with_size(1024, 768)
            .with_label(&format!("NAND Dump Viewer - {}", filename));

        let tabs = Tabs::default().with_size(1024, 768).with_pos(0, 0);

        // --- TAB 1: NAND Dump Viewer ---
        let mut tab_viewer = Group::default()
            .with_size(1024, 738)
            .with_pos(0, 30)
            .with_label("NAND Viewer\t");

        let mut viewport_frame = Frame::default()
            .with_size(1024, 648)
            .with_pos(0, 30);
        viewport_frame.set_frame(fltk::enums::FrameType::FlatBox);

        // Create status bar
        let mut status_bar = TextEditor::default()
            .with_size(1024, 60)
            .with_pos(0, 678);
        status_bar.set_buffer(fltk::text::TextBuffer::default());

        tab_viewer.end();

        // --- TAB 2: Workflow Editor (embedded egui GL Canvas) ---
        let tab_workflow = Group::default()
            .with_size(1024, 738)
            .with_pos(0, 30)
            .with_label("Workflow Editor\t");

        let mut gl_win = GlWindow::default()
            .with_size(1024, 738)
            .with_pos(0, 30);
        gl_win.set_mode(fltk::enums::Mode::Opengl3);
        gl_win.end();

        tab_workflow.end();

        // --- TAB 3: Search (embedded egui GL Canvas) ---
        let tab_search = Group::default()
            .with_size(1024, 738)
            .with_pos(0, 30)
            .with_label("Search\t");

        let mut gl_search = GlWindow::default()
            .with_size(1024, 738)
            .with_pos(0, 30);
        gl_search.set_mode(fltk::enums::Mode::Opengl3);
        gl_search.end();

        tab_search.end();

        // --- TAB 4: Hex Viewer (embedded egui GL Canvas) ---
        let tab_hex = Group::default()
            .with_size(1024, 738)
            .with_pos(0, 30)
            .with_label("Hex\t");

        let mut gl_hex = GlWindow::default()
            .with_size(1024, 738)
            .with_pos(0, 30);
        gl_hex.set_mode(fltk::enums::Mode::Opengl3);
        gl_hex.end();

        tab_hex.end();

        // --- TAB 5: Page Structure Editor (embedded egui GL Canvas) ---
        let tab_page_structure = Group::default()
            .with_size(1024, 738)
            .with_pos(0, 30)
            .with_label("Page Structure\t");

        let mut gl_page_structure = GlWindow::default()
            .with_size(1024, 738)
            .with_pos(0, 30);
        gl_page_structure.set_mode(fltk::enums::Mode::Opengl3);
        gl_page_structure.end();

        tab_page_structure.end();

        tabs.end();

        if file_loader.is_none() {
            tab_viewer.deactivate();
            let mut tabs_mut = tabs.clone();
            let _ = tabs_mut.set_value(&tab_workflow);
        }

        window.resizable(&tabs);
        window.end();
        window.show();

        // Setup fltk-egui for workflow tab with lazy initialization on first draw
        let workflow_state = Arc::new(Mutex::new(
            initial_workflow.unwrap_or_else(|| WorkflowEditorState::new(Some(&metadata.path)))
        ));
        let egui_state: Arc<Mutex<Option<(Painter, EguiState)>>> = Arc::new(Mutex::new(None));
        let egui_ctx = egui::Context::default();
        egui_ctx.set_fonts(chakra_font_definitions());
        let workflow_state_draw = workflow_state.clone();
        let egui_state_draw = egui_state.clone();
        let egui_ctx_draw = egui_ctx.clone();

        gl_win.draw(move |w| {
            if !w.shown() {
                return;
            }
            w.make_current();
            let mut state_guard = egui_state_draw.lock().unwrap();
            if state_guard.is_none() {
                *state_guard = Some(fltk_egui::init(w));
            }
            if let Some((painter, state)) = state_guard.as_mut() {
                let raw_input = state.take_input();
                let ppp = state.pixels_per_point();

                let full_output = egui_ctx_draw.run(raw_input, |ctx| {
                    let mut wf = workflow_state_draw.lock().unwrap();
                    wf.show_ui(ctx);
                });

                state.fuse_output(w, full_output.platform_output);
                let clipped_primitives = egui_ctx_draw.tessellate(full_output.shapes, full_output.pixels_per_point);

                painter.paint_and_update_textures(
                    [w.width() as u32, w.height() as u32],
                    ppp,
                    &clipped_primitives,
                    &full_output.textures_delta,
                );
                w.swap_buffers();
            }
        });

        // Directly attach event handler to gl_win and set focus to ensure it receives mouse events
        let egui_state_handle = egui_state.clone();
        gl_win.handle(move |w, ev| {
            // Forward FLTK events to egui state
            if let Ok(mut state_guard) = egui_state_handle.try_lock() {
                if let Some((_, state)) = state_guard.as_mut() {
                    state.fuse_input(w, ev);
                }
            }
            // Request redraw for relevant events
            match ev {
                Event::Push => {
                    let _ = w.take_focus();
                    w.redraw();
                    true
                }
                Event::Focus | Event::Unfocus => {
                    // Accept/release keyboard focus so FLTK delivers KeyDown/KeyUp events
                    w.redraw();
                    true
                }
                Event::Drag | Event::Move | Event::Released | Event::KeyDown | Event::KeyUp | Event::MouseWheel | Event::Resize => {
                    w.redraw();
                    true
                }
                _ => false,
            }
        });
        // Ensure the GL window can gain focus to receive keyboard and mouse input
        gl_win.set_visible_focus();

        // Setup fltk-egui for search tab
        let search_tab_state = Arc::new(Mutex::new(SearchTabState::new()));
        let search_egui_state: Arc<Mutex<Option<(Painter, EguiState)>>> = Arc::new(Mutex::new(None));
        let search_egui_ctx = egui::Context::default();
        search_egui_ctx.set_fonts(chakra_font_definitions());

        let search_tab_draw = search_tab_state.clone();
        let search_egui_state_draw = search_egui_state.clone();
        let search_egui_ctx_draw = search_egui_ctx.clone();
        let workflow_for_search = workflow_state.clone();

        gl_search.draw(move |w| {
            if !w.shown() {
                return;
            }
            w.make_current();
            let mut state_guard = search_egui_state_draw.lock().unwrap();
            if state_guard.is_none() {
                *state_guard = Some(fltk_egui::init(w));
            }
            if let Some((painter, state)) = state_guard.as_mut() {
                let raw_input = state.take_input();
                let ppp = state.pixels_per_point();

                let (target_name, provider_opt) = {
                    let wf = workflow_for_search.lock().unwrap();
                    match wf.build_search_provider() {
                        Ok((_id, name, prov)) => (name, Some(prov)),
                        Err(e) => (format!("None ({})", e), None),
                    }
                };

                let full_output = search_egui_ctx_draw.run(raw_input, |ctx| {
                    let mut st = search_tab_draw.lock().unwrap();
                    st.show_ui(ctx, &target_name, provider_opt);
                });

                state.fuse_output(w, full_output.platform_output);
                let clipped_primitives = search_egui_ctx_draw.tessellate(full_output.shapes, full_output.pixels_per_point);

                painter.paint_and_update_textures(
                    [w.width() as u32, w.height() as u32],
                    ppp,
                    &clipped_primitives,
                    &full_output.textures_delta,
                );
                w.swap_buffers();
            }
        });

        let search_egui_handle = search_egui_state.clone();
        let mut gl_search_handle = gl_search.clone();
        gl_search_handle.handle(move |w, ev| {
            if let Ok(mut state_guard) = search_egui_handle.try_lock() {
                if let Some((_, state)) = state_guard.as_mut() {
                    state.fuse_input(w, ev);
                }
            }
            match ev {
                Event::Push => {
                    // Request keyboard focus when user clicks inside the GL window
                    let _ = w.take_focus();
                    w.redraw();
                    true
                }
                Event::Focus | Event::Unfocus => {
                    // Accept/release keyboard focus so FLTK delivers KeyDown/KeyUp events
                    w.redraw();
                    true
                }
                Event::Drag | Event::Move | Event::Released | Event::KeyDown | Event::KeyUp | Event::MouseWheel | Event::Resize => {
                    w.redraw();
                    true
                }
                _ => false,
            }
        });
        gl_search.set_visible_focus();

        // Setup page structure state early so it can be shared with the hex tab draw closure
        let page_structure_state = Arc::new(Mutex::new(PageStructureTabState::new()));

        // Setup fltk-egui for hex tab
        let hex_tab_state = Arc::new(Mutex::new(HexTabState::new()));
        let hex_egui_state: Arc<Mutex<Option<(Painter, EguiState)>>> = Arc::new(Mutex::new(None));
        let hex_egui_ctx = egui::Context::default();
        hex_egui_ctx.set_fonts(chakra_font_definitions());

        let hex_tab_draw = hex_tab_state.clone();
        let hex_egui_state_draw = hex_egui_state.clone();
        let hex_egui_ctx_draw = hex_egui_ctx.clone();
        let workflow_for_hex = workflow_state.clone();
        let search_tab_for_hex = search_tab_state.clone();
        let page_structure_for_hex = page_structure_state.clone();

        gl_hex.draw(move |w| {
            if !w.shown() {
                return;
            }
            w.make_current();
            let mut state_guard = hex_egui_state_draw.lock().unwrap();
            if state_guard.is_none() {
                *state_guard = Some(fltk_egui::init(w));
            }
            if let Some((painter, state)) = state_guard.as_mut() {
                let raw_input = state.take_input();
                let ppp = state.pixels_per_point();

                let (main_prov_opt, raw_prov_opt) = {
                    let wf = workflow_for_hex.lock().unwrap();
                    match wf.build_hex_providers() {
                        Ok((main_p, raw_p)) => (Some(main_p), raw_p),
                        Err(_) => (None, None),
                    }
                };

                let (search_results_clone, pattern_len) = {
                    if let Ok(st) = search_tab_for_hex.try_lock() {
                        let pat_bytes = crate::search::parse_pattern(&st.options).unwrap_or_default();
                        (st.results.clone(), pat_bytes.len())
                    } else {
                        (Vec::new(), 0)
                    }
                };

                let page_records_clone = {
                    if let Ok(ps) = page_structure_for_hex.try_lock() {
                        ps.to_page_records()
                    } else {
                        Vec::new()
                    }
                };

                let full_output = hex_egui_ctx_draw.run(raw_input, |ctx| {
                    let mut ht = hex_tab_draw.lock().unwrap();
                    ht.show_ui(
                        ctx,
                        main_prov_opt.as_ref(),
                        raw_prov_opt.as_ref(),
                        &search_results_clone,
                        pattern_len,
                        &page_records_clone,
                    );
                });

                state.fuse_output(w, full_output.platform_output);
                let clipped_primitives = hex_egui_ctx_draw.tessellate(full_output.shapes, full_output.pixels_per_point);

                painter.paint_and_update_textures(
                    [w.width() as u32, w.height() as u32],
                    ppp,
                    &clipped_primitives,
                    &full_output.textures_delta,
                );
                w.swap_buffers();
            }
        });

        let hex_egui_handle = hex_egui_state.clone();
        let mut gl_hex_handle = gl_hex.clone();
        gl_hex_handle.handle(move |w, ev| {
            if let Ok(mut state_guard) = hex_egui_handle.try_lock() {
                if let Some((_, state)) = state_guard.as_mut() {
                    state.fuse_input(w, ev);
                }
            }
            match ev {
                Event::Push => {
                    let _ = w.take_focus();
                    w.redraw();
                    true
                }
                Event::Focus | Event::Unfocus => {
                    w.redraw();
                    true
                }
                Event::Drag | Event::Move | Event::Released | Event::KeyDown | Event::KeyUp | Event::MouseWheel | Event::Resize => {
                    w.redraw();
                    true
                }
                _ => false,
            }
        });
        gl_hex.set_visible_focus();

        // Setup fltk-egui for page structure tab
        let ps_egui_state: Arc<Mutex<Option<(Painter, EguiState)>>> = Arc::new(Mutex::new(None));
        let ps_egui_ctx = egui::Context::default();
        ps_egui_ctx.set_fonts(chakra_font_definitions());

        let ps_tab_draw = page_structure_state.clone();
        let ps_egui_state_draw = ps_egui_state.clone();
        let ps_egui_ctx_draw = ps_egui_ctx.clone();
        let workflow_for_ps = workflow_state.clone();

        gl_page_structure.draw(move |w| {
            if !w.shown() {
                return;
            }
            w.make_current();
            let mut state_guard = ps_egui_state_draw.lock().unwrap();
            if state_guard.is_none() {
                *state_guard = Some(fltk_egui::init(w));
            }
            if let Some((painter, state)) = state_guard.as_mut() {
                let raw_input = state.take_input();
                let ppp = state.pixels_per_point();

                let active_page_length = {
                    let wf = workflow_for_ps.lock().unwrap();
                    wf.active_page_length()
                };

                let full_output = ps_egui_ctx_draw.run(raw_input, |ctx| {
                    let mut ps = ps_tab_draw.lock().unwrap();
                    ps.show_ui(ctx, active_page_length);
                });

                state.fuse_output(w, full_output.platform_output);
                let clipped_primitives = ps_egui_ctx_draw.tessellate(full_output.shapes, full_output.pixels_per_point);

                painter.paint_and_update_textures(
                    [w.width() as u32, w.height() as u32],
                    ppp,
                    &clipped_primitives,
                    &full_output.textures_delta,
                );
                w.swap_buffers();
            }
        });

        let ps_egui_handle = ps_egui_state.clone();
        let mut gl_ps_handle = gl_page_structure.clone();
        gl_ps_handle.handle(move |w, ev| {
            if let Ok(mut state_guard) = ps_egui_handle.try_lock() {
                if let Some((_, state)) = state_guard.as_mut() {
                    state.fuse_input(w, ev);
                }
            }
            match ev {
                Event::Push => {
                    let _ = w.take_focus();
                    w.redraw();
                    true
                }
                Event::Focus | Event::Unfocus => {
                    w.redraw();
                    true
                }
                Event::Drag | Event::Move | Event::Released | Event::KeyDown | Event::KeyUp | Event::MouseWheel | Event::Resize => {
                    w.redraw();
                    true
                }
                _ => false,
            }
        });
        gl_page_structure.set_visible_focus();

        // Create viewport manager
        let viewport_manager = Arc::new(Mutex::new(ViewportManager::new(
            metadata.clone(),
            task_queue.clone(),
        )));

        // Tab switch callback to immediately redraw OpenGL contexts and sync positions
        let mut tabs_switch = tabs.clone();
        let tabs_for_cb = tabs_switch.clone();
        let mut gl_win_switch = gl_win.clone();
        let mut gl_search_switch = gl_search.clone();
        let mut gl_hex_switch = gl_hex.clone();
        let mut gl_page_structure_switch = gl_page_structure.clone();
        let hex_tab_switch = hex_tab_state.clone();
        let viewport_manager_tab_switch = viewport_manager.clone();
        let metadata_tab_switch = metadata.clone();

        tabs_switch.set_callback(move |_| {
            gl_win_switch.redraw();
            gl_search_switch.redraw();
            gl_hex_switch.redraw();
            gl_page_structure_switch.redraw();
            // Set keyboard focus to the appropriate GL window when its tab becomes active.
            if let Some(active) = tabs_for_cb.value() {
                let lbl = active.label();
                let trimmed = lbl.trim();
                if trimmed.starts_with("Workflow") {
                    gl_win_switch.set_visible_focus();
                } else if trimmed.starts_with("Search") {
                    gl_search_switch.set_visible_focus();
                } else if trimmed.starts_with("Page") {
                    gl_page_structure_switch.set_visible_focus();
                } else if trimmed.starts_with("Hex") {
                    gl_hex_switch.set_visible_focus();
                    // Synchronize NAND viewer position to Hex Tab
                    let pl = metadata_tab_switch.page_length as u64;
                    let bs = metadata_tab_switch.block_size as u64;
                    let gh = metadata_tab_switch.grid_height as u64;
                    if pl > 0 && bs > 0 && gh > 0 {
                        if let Ok(vm) = viewport_manager_tab_switch.try_lock() {
                            let vp = vm.get_viewport();
                            let scale = 2.0_f64.powi(vp.level);
                            let pixel_x_l0 = (vp.center_x * scale).max(0.0) as u64;
                            let pixel_y_l0 = (vp.center_y * scale).max(0.0) as u64;
                            let block_width_pixels = pl * 8;
                            let block_height_pixels = bs;
                            let block_x = pixel_x_l0 / block_width_pixels;
                            let block_y = pixel_y_l0 / block_height_pixels;
                            let block = block_x * gh + block_y;
                            let page = pixel_y_l0 % block_height_pixels;
                            let byte_in_page = (pixel_x_l0 % block_width_pixels) / 8;
                            let block_stride = pl * bs;
                            let offset = block * block_stride + page * pl + byte_in_page;
                            if let Ok(mut ht) = hex_tab_switch.try_lock() {
                                ht.pending_jump_offset = Some(offset);
                            }
                        }
                    }
                } else if trimmed.starts_with("NAND") {
                    // Synchronize Hex Tab position back to NAND viewer
                    let pl = metadata_tab_switch.page_length as u64;
                    let bs = metadata_tab_switch.block_size as u64;
                    let gh = metadata_tab_switch.grid_height as u64;
                    if pl > 0 && bs > 0 && gh > 0 {
                        if let Ok(ht) = hex_tab_switch.try_lock() {
                            let block = if bs > 0 { ht.current_page / bs } else { 0 };
                            let page = if bs > 0 { ht.current_page % bs } else { 0 };
                            let byte_in_page = ht.col_offset_in_page;

                            let block_x = block / gh;
                            let block_y = block % gh;
                            let pixel_x_l0 = (block_x * (pl * 8) + (byte_in_page * 8)) as f64;
                            let pixel_y_l0 = (block_y * bs + page) as f64;

                            if let Ok(mut vm) = viewport_manager_tab_switch.try_lock() {
                                let vp = vm.get_viewport().clone();
                                let scale = 2.0_f64.powi(vp.level);
                                let center_x = pixel_x_l0 / scale;
                                let center_y = pixel_y_l0 / scale;
                                vm.update_viewport(vp.level, center_x, center_y, vp.width_pixels, vp.height_pixels);
                                vm.update_task_priorities();
                            }
                        }
                    }
                }
            }
        });

        // Create zoom controller
        let zoom_controller = Arc::new(Mutex::new(ZoomController::new(
            metadata.clone(),
            viewport_manager.clone(),
            1024,
            648, // Account for status bar height
        )));

        // Create pan controller
        let pan_controller = Arc::new(Mutex::new(PanController::new(
            metadata.clone(),
            viewport_manager.clone(),
            1024,
            648,
        )));

        // Create address display
        let address_display = Arc::new(Mutex::new(AddressDisplay::new()));

        // Create viewport renderer
        let viewport_renderer = if let Some(ref fl) = file_loader {
            Arc::new(ViewportRenderer::with_file_loader(cache.clone(), TILE_WIDTH, TILE_HEIGHT, fl.clone(), metadata.clone()))
        } else {
            Arc::new(ViewportRenderer::new(cache.clone(), TILE_WIDTH, TILE_HEIGHT))
        };

        // Initialize viewport at upper left corner with default zoom (level 0)
        {
            let mut vm = viewport_manager.lock().unwrap();
            let center_x = 1024.0 / 2.0;
            let center_y = 648.0 / 2.0;
            vm.update_viewport(0, center_x, center_y, 1024, 648);
            vm.update_task_priorities();
        }

        // Create initial viewport image
        let viewport_image = Arc::new(Mutex::new(RgbaImage::new(1024, 648)));
        
        // Create FLTK tile cache
        let fltk_tile_cache = Arc::new(Mutex::new(HashMap::new()));

        let status_bar_arc = Arc::new(Mutex::new(status_bar));
        let viewport_frame_arc = Arc::new(Mutex::new(viewport_frame));
        
        // Clone metadata for event handlers before moving into struct
        let metadata_for_events = metadata.clone();

        let app_window = AppWindow {
            window,
            workflow_state: workflow_state.clone(),
            search_tab_state: search_tab_state.clone(),
            hex_tab_state: hex_tab_state.clone(),
            page_structure_tab_state: page_structure_state.clone(),
            viewport_frame: viewport_frame_arc.clone(),
            viewport_manager: viewport_manager.clone(),
            zoom_controller: zoom_controller.clone(),
            pan_controller: pan_controller.clone(),
            address_display,
            metadata: metadata.clone(),
            task_queue,
            cache: cache.clone(),
            viewport_renderer: viewport_renderer.clone(),
            viewport_image: viewport_image.clone(),
            fltk_tile_cache: fltk_tile_cache.clone(),
            mouse_x: 0,
            mouse_y: 0,
            status_bar: status_bar_arc.clone(),
            last_render_time: Arc::new(Mutex::new(None)),
            last_render_duration: Arc::new(Mutex::new(0.0)),
        };

        // Set up mouse event handlers
        let mut frame_for_events = viewport_frame_arc.lock().unwrap().clone();
        let viewport_image_for_events = viewport_image.clone();
        let viewport_frame_for_events = viewport_frame_arc.clone();
        let viewport_renderer_for_events = viewport_renderer.clone();
        let viewport_manager_for_events = viewport_manager.clone();
        let pan_controller_for_events = pan_controller.clone();
        let zoom_controller_for_events = zoom_controller.clone();
        let status_bar_for_events = status_bar_arc.clone();
        let address_display_for_events = app_window.address_display.clone();
        let last_render_time_frame = Arc::new(Mutex::new(None));
        let last_render_duration_frame = Arc::new(Mutex::new(0.0));
        
        let mut last_mouse_x = 0;
        let mut last_mouse_y = 0;
        let mut is_dragging = false;
        let mut last_update = Instant::now();
        let last_render_time_for_events = last_render_time_frame.clone();
        let last_render_duration_for_events = last_render_duration_frame.clone();
        
        frame_for_events.handle(move |_frame, event| {
            match event {
                Event::Move => {
                    // Track mouse movement for address display
                    let mouse_x = fltk::app::event_x();
                    let mouse_y = fltk::app::event_y();
                    
                    // Update address display
                    let viewport = viewport_manager_for_events.lock().unwrap();
                    let vp = viewport.get_viewport().clone();
                    drop(viewport);
                    
                    // Calculate address at mouse position
                    {
                        let mut addr_display = AddressDisplay::new();
                        addr_display.update_mouse_position(
                            mouse_x as u32,
                            mouse_y as u32,
                            &vp,
                            &metadata_for_events,
                        );
                        // Update the shared address display
                        let mut shared_addr = address_display_for_events.lock().unwrap();
                        *shared_addr = addr_display;
                    }
                    
                    // Get last render time and duration for display
                    let (render_duration, time_since_last_render) = {
                        let last_time = last_render_time_for_events.lock().unwrap();
                        let last_duration = last_render_duration_for_events.lock().unwrap();
                        let now = Instant::now();
                        let time_since = last_time.map(|t| now.duration_since(t).as_secs_f64() * 1000.0).unwrap_or(0.0);
                        (*last_duration, time_since)
                    };
                    
                    // Update status bar using unified function
                    if let Ok(status_bar) = status_bar_for_events.lock() {
                        if let Some(mut buf) = status_bar.buffer() {
                            let addr_display = address_display_for_events.lock().unwrap();
                            let address_str = addr_display.get_address();
                            drop(addr_display);
                            
                            let zoom_factor = {
                                let zoom = zoom_controller_for_events.lock().unwrap();
                                zoom.get_zoom_factor()
                            };
                            
                            let half_width = (vp.width_pixels as f64) / 2.0;
                            let half_height = (vp.height_pixels as f64) / 2.0;
                            let left = (vp.center_x - half_width).max(0.0) as u64;
                            let right = (vp.center_x + half_width) as u64;
                            let top = (vp.center_y - half_height).max(0.0) as u64;
                            let bottom = (vp.center_y + half_height) as u64;
                            
                            buf.set_text(&format!(
                                "Address: {}\nRender: {:.1}ms | Since last: {:.1}ms | Zoom: {:.3}x | Level: {} | Viewport: ({}, {}) - ({}, {})",
                                address_str, render_duration, time_since_last_render, zoom_factor, vp.level, left, top, right, bottom
                            ));
                        }
                    }
                    
                    true
                }
                Event::Push => {
                    // Check for mouse button
                    let button = fltk::app::event_button();
                    
                    if button == 1 {  // Left button - open URL in browser
                        let addr_display = address_display_for_events.lock().unwrap();
                        if let Some((block, page, byte, _bit)) = addr_display.get_address_components() {
                            let metadata = &metadata_for_events;
                            let _ = open_xorviewer_in_browser(metadata, block as u64, page as u64, byte as u64);
                        }
                        return true;
                    } else if button == 3 {  // Right button - pan
                        is_dragging = true;
                        last_mouse_x = fltk::app::event_x();
                        last_mouse_y = fltk::app::event_y();
                        last_update = Instant::now();
                        log::debug!("Pan started");
                        return true;
                    }
                    false
                }
                Event::Drag => {
                    // Right mouse button drag for panning
                    if is_dragging {
                        let current_x = fltk::app::event_x();
                        let current_y = fltk::app::event_y();
                        let dx = current_x - last_mouse_x;
                        let dy = current_y - last_mouse_y;
                        
                        // Throttle updates to max 60 FPS (16.6ms between updates)
                        let elapsed = last_update.elapsed();
                        if elapsed.as_millis() < 16 {
                            return true; // Skip this update
                        }
                        
                        let render_start = Instant::now();
                        log::debug!("Drag event: dx={}, dy={}, elapsed={}ms", dx, dy, elapsed.as_millis());
                        
                        // Pan the viewport
                        {
                            let mut pan_ctrl = pan_controller_for_events.lock().unwrap();
                            pan_ctrl.pan(dx as f64, dy as f64);
                        }
                        
                        // Re-render viewport with smooth blending
                        let viewport = viewport_manager_for_events.lock().unwrap();
                        let vp = viewport.get_viewport().clone();
                        viewport.update_task_priorities();
                        drop(viewport);
                        
                        // Get blend parameters from zoom controller
                        let zoom = zoom_controller_for_events.lock().unwrap();
                        let blend_level = zoom.get_next_level();
                        let blend_factor = zoom.get_blend_factor();
                        drop(zoom);
                        
                        let rendered_image = viewport_renderer_for_events.render_viewport(&vp, blend_level, blend_factor);
                        let mut viewport_img = viewport_image_for_events.lock().unwrap();
                        *viewport_img = rendered_image;
                        
                        let width = viewport_img.width() as i32;
                        let height = viewport_img.height() as i32;
                        let raw_data = viewport_img.as_raw().clone();
                        drop(viewport_img);
                        
                        if let Ok(fltk_img) = fltk::image::RgbImage::new(&raw_data, width, height, ColorDepth::Rgba8) {
                            let mut frame = viewport_frame_for_events.lock().unwrap();
                            frame.set_image(Some(fltk_img));
                            frame.redraw();
                            drop(frame);
                            fltk::app::awake(); // Force event loop to process
                        }
                        
                        // Calculate render time and time since last render
                        let render_duration = render_start.elapsed().as_secs_f64() * 1000.0;
                        let time_since_last_render = {
                            let mut last_time = last_render_time_for_events.lock().unwrap();
                            let now = Instant::now();
                            let time_since = last_time.map(|t| now.duration_since(t).as_secs_f64() * 1000.0).unwrap_or(0.0);
                            *last_time = Some(now);
                            time_since
                        };
                        
                        // Update last render duration
                        {
                            let mut last_duration = last_render_duration_for_events.lock().unwrap();
                            *last_duration = render_duration;
                        }
                        
                        // Update status bar using unified function
                        if let Ok(status_bar) = status_bar_for_events.lock() {
                            if let Some(mut buf) = status_bar.buffer() {
                                let addr_display = address_display_for_events.lock().unwrap();
                                let address_str = addr_display.get_address();
                                drop(addr_display);
                                
                                let zoom_factor = {
                                    let zoom = zoom_controller_for_events.lock().unwrap();
                                    zoom.get_zoom_factor()
                                };
                                
                                let half_width = (vp.width_pixels as f64) / 2.0;
                                let half_height = (vp.height_pixels as f64) / 2.0;
                                let left = (vp.center_x - half_width).max(0.0) as u64;
                                let right = (vp.center_x + half_width) as u64;
                                let top = (vp.center_y - half_height).max(0.0) as u64;
                                let bottom = (vp.center_y + half_height) as u64;
                                
                                buf.set_text(&format!(
                                    "Address: {}\nRender: {:.1}ms | Since last: {:.1}ms | Zoom: {:.3}x | Level: {} | Viewport: ({}, {}) - ({}, {})", 
                                    address_str,
                                    render_duration,
                                    time_since_last_render,
                                    zoom_factor,
                                    vp.level,
                                    left, top, right, bottom
                                ));
                            }
                        }
                        
                        // Log render times
                        log::info!("Render time: {:.1}ms | Time since last render: {:.1}ms", render_duration, time_since_last_render);
                        
                        last_mouse_x = current_x;
                        last_mouse_y = current_y;
                        last_update = Instant::now();
                        
                        return true;
                    }
                    false
                }
                Event::Released => {
                    // Mouse button released
                    is_dragging = false;
                    true
                }
                _ => false,
            }
        });

        // Perform initial render
        app_window.composite_tiles_into_viewport();
        app_window.draw_viewport_to_frame();
        
        // Set up unified timer loop (tile completion, search jump navigation, workflow output sync)
        let viewport_image_clone = app_window.viewport_image.clone();
        let viewport_frame_clone = app_window.viewport_frame.clone();
        let viewport_renderer_clone = app_window.viewport_renderer.clone();
        let viewport_manager_clone = app_window.viewport_manager.clone();
        let zoom_controller_clone = app_window.zoom_controller.clone();
        let search_tab_timer = app_window.search_tab_state.clone();
        let hex_tab_timer = app_window.hex_tab_state.clone();
        let workflow_state_timer = app_window.workflow_state.clone();
        let mut tabs_timer = tabs.clone();
        let mut tab_viewer_timer = tab_viewer.clone();
        let mut gl_search_timer = gl_search.clone();
        let mut gl_hex_timer = gl_hex.clone();
        let fltk_tile_cache_timer = app_window.fltk_tile_cache.clone();
        let initial_identity = file_loader.as_ref().map(|fl| fl.lock().cache_identity()).unwrap_or_default();
        let current_identity_timer = Arc::new(Mutex::new(initial_identity));
        let metadata_timer = Arc::new(Mutex::new(metadata.clone()));

        // Channel for tile receiver bridge
        let (s, r) = fltk::app::channel::<TileCoord>();
        if let Some(rx) = tile_rx {
            std::thread::spawn(move || {
                while let Ok(coord) = rx.recv() {
                    let _ = s.send(coord);
                }
            });
        }

        fltk::app::add_timeout3(0.05, move |handle| {
            let mut need_redraw = false;

            // 1. Check for tile completion messages
            while let Some(_coord) = r.recv() {
                need_redraw = true;
            }

            // 2. Check if Search tab requested a jump to match or has live updates
            if let Ok(mut st) = search_tab_timer.try_lock() {
                let had_new = st.poll_results();
                if st.is_searching || had_new {
                    gl_search_timer.redraw();
                }
                if let Some((block, page, offset)) = st.jump_target.take() {
                    let _ = tabs_timer.set_value(&tab_viewer_timer);
                    tabs_timer.redraw();
                    tab_viewer_timer.redraw();

                    let (pl, bs, gh) = {
                        let m = metadata_timer.lock().unwrap();
                        (m.page_length as u64, m.block_size as u64, m.grid_height as u64)
                    };
                    if let Ok(mut ht) = hex_tab_timer.try_lock() {
                        let block_stride = pl * bs;
                        let byte_off = block * block_stride + page * pl + offset;
                        ht.pending_jump_offset = Some(byte_off);
                    }
                    if gh > 0 && pl > 0 && bs > 0 {
                        let block_x = block / gh;
                        let block_y = block % gh;
                        let pixel_x_l0 = (block_x * (pl * 8) + (offset * 8)) as f64;
                        let pixel_y_l0 = (block_y * bs + page) as f64;

                        let mut vm = viewport_manager_clone.lock().unwrap();
                        let vp = vm.get_viewport().clone();
                        let scale = 2.0_f64.powi(vp.level);
                        let center_x = pixel_x_l0 / scale;
                        let center_y = pixel_y_l0 / scale;
                        vm.update_viewport(vp.level, center_x, center_y, vp.width_pixels, vp.height_pixels);
                        vm.update_task_priorities();
                    }
                    need_redraw = true;
                }
            }

            // 2b. Check if Hex tab requested a jump to NAND viewer
            if let Ok(mut ht) = hex_tab_timer.try_lock() {
                if let Some((block, page, offset)) = ht.jump_to_nand.take() {
                    let _ = tabs_timer.set_value(&tab_viewer_timer);
                    tabs_timer.redraw();
                    tab_viewer_timer.redraw();

                    let (pl, bs, gh) = {
                        let m = metadata_timer.lock().unwrap();
                        (m.page_length as u64, m.block_size as u64, m.grid_height as u64)
                    };
                    if gh > 0 && pl > 0 && bs > 0 {
                        let block_x = block / gh;
                        let block_y = block % gh;
                        let pixel_x_l0 = (block_x * (pl * 8) + (offset * 8)) as f64;
                        let pixel_y_l0 = (block_y * bs + page) as f64;

                        let mut vm = viewport_manager_clone.lock().unwrap();
                        let vp = vm.get_viewport().clone();
                        let scale = 2.0_f64.powi(vp.level);
                        let center_x = pixel_x_l0 / scale;
                        let center_y = pixel_y_l0 / scale;
                        vm.update_viewport(vp.level, center_x, center_y, vp.width_pixels, vp.height_pixels);
                        vm.update_task_priorities();
                    }
                    need_redraw = true;
                }
            }

            // 3. Check if active OutputViewer in workflow changed (Variante A)
            if let Ok(wf) = workflow_state_timer.try_lock() {
                match wf.build_active_output_provider() {
                    Ok(provider) => {
                        let new_ident = provider.lock().cache_identity();
                        let mut cur_ident = current_identity_timer.lock().unwrap();
                        if *cur_ident != new_ident {
                            log::info!("Switching ZoomViewer to new active workflow output: {}", new_ident);
                            *cur_ident = new_ident.clone();
                            let new_meta = provider.lock().get_metadata();
                            *metadata_timer.lock().unwrap() = new_meta.clone();

                            if let Ok(new_cache) = CacheManager::new(".cache", new_ident) {
                                viewport_renderer_clone.update_provider(provider, Arc::new(new_cache), new_meta.clone());
                            }
                            if let Ok(mut tc) = fltk_tile_cache_timer.try_lock() {
                                tc.clear();
                            }

                            if let Ok(mut vm) = viewport_manager_clone.try_lock() {
                                let center_x = 1024.0 / 2.0;
                                let center_y = 648.0 / 2.0;
                                vm.update_viewport(0, center_x, center_y, 1024, 648);
                                vm.update_task_priorities();
                            }
                            tab_viewer_timer.activate();
                            gl_hex_timer.redraw();
                            need_redraw = true;
                        }
                    }
                    Err(_) => {
                        let mut cur_ident = current_identity_timer.lock().unwrap();
                        if !cur_ident.is_empty() {
                            log::info!("No active workflow output provider available: disabling ZoomViewer tab");
                            *cur_ident = String::new();
                            tab_viewer_timer.deactivate();
                        }
                    }
                }
            }

            if need_redraw {
                let vp = viewport_manager_clone.lock().unwrap().get_viewport().clone();
                let zoom = zoom_controller_clone.lock().unwrap();
                let blend_level = zoom.get_next_level();
                let blend_factor = zoom.get_blend_factor();
                drop(zoom);

                let rendered_image = viewport_renderer_clone.render_viewport(&vp, blend_level, blend_factor);
                let mut viewport_img = viewport_image_clone.lock().unwrap();
                *viewport_img = rendered_image;

                let width = viewport_img.width() as i32;
                let height = viewport_img.height() as i32;
                let raw_data = viewport_img.as_raw().clone();
                drop(viewport_img);

                if let Ok(fltk_img) = fltk::image::RgbImage::new(&raw_data, width, height, ColorDepth::Rgba8) {
                    let mut frame = viewport_frame_clone.lock().unwrap();
                    frame.set_image(Some(fltk_img));
                    frame.redraw();
                }
            }

            // Repeat timer
            fltk::app::repeat_timeout3(0.05, handle);
        });

        // Set up window event handler (resize, close, mouse wheel)
        {
            let viewport_frame_resize = viewport_frame_arc.clone();
            let status_bar_resize = status_bar_arc.clone();
            let viewport_manager_resize = viewport_manager.clone();
            let zoom_controller_resize = zoom_controller.clone();
            let pan_controller_resize = pan_controller.clone();
            let viewport_image_wheel = viewport_image.clone();
            let viewport_renderer_wheel = viewport_renderer.clone();
            let zoom_controller_wheel = zoom_controller.clone();
            let status_bar_wheel = status_bar_arc.clone();
            let address_display_wheel = app_window.address_display.clone();
            let last_render_time_wheel = last_render_time_frame.clone();
            let last_render_duration_wheel = last_render_duration_frame.clone();
            let mut window_clone = app_window.window.clone();
            
            window_clone.handle(move |win, event| {
                match event {
                    fltk::enums::Event::Resize => {
                        let new_width = win.width() as u32;
                        let new_height = win.height() as u32;
                        let status_bar_height = 60u32;
                        let viewport_height = new_height.saturating_sub(status_bar_height);
                        
                        // Resize viewport frame
                        if let Ok(mut frame) = viewport_frame_resize.lock() {
                            frame.resize(0, 0, new_width as i32, viewport_height as i32);
                        }
                        
                        // Resize and reposition status bar
                        if let Ok(mut status) = status_bar_resize.lock() {
                            status.resize(0, viewport_height as i32, new_width as i32, status_bar_height as i32);
                        }
                        
                        // Update viewport manager with new dimensions
                        if let Ok(mut vm) = viewport_manager_resize.lock() {
                            let (level, center_x, center_y) = {
                                let vp = vm.get_viewport();
                                (vp.level, vp.center_x, vp.center_y)
                            };
                            vm.update_viewport(
                                level,
                                center_x,
                                center_y,
                                new_width,
                                viewport_height,
                            );
                        }
                        
                        // Update zoom controller with new screen dimensions
                        if let Ok(mut zc) = zoom_controller_resize.lock() {
                            zc.update_screen_dimensions(new_width, viewport_height);
                        }
                        
                        // Update pan controller with new screen dimensions
                        if let Ok(mut pc) = pan_controller_resize.lock() {
                            pc.update_screen_dimensions(new_width, viewport_height);
                        }
                        
                        win.redraw();
                        true
                    }
                    fltk::enums::Event::MouseWheel => {
                        log::debug!("MouseWheel event at window level");
                        let mouse_x = fltk::app::event_x() as f64;
                        let mouse_y = fltk::app::event_y() as f64;
                        let scroll_amount = fltk::app::event_dy();
                        
                        log::debug!("Mouse at ({}, {}), scroll: {:?}", mouse_x, mouse_y, scroll_amount);
                        
                        let render_start = Instant::now();
                        
                        {
                            let mut zoom_ctrl = zoom_controller_resize.lock().unwrap();
                            match scroll_amount {
                                MouseWheel::Up => {
                                    log::debug!("Zooming in");
                                    zoom_ctrl.zoom_in(mouse_x, mouse_y);
                                }
                                MouseWheel::Down => {
                                    log::debug!("Zooming out");
                                    zoom_ctrl.zoom_out(mouse_x, mouse_y);
                                }
                                _ => {}
                            }
                        }
                        
                        // Re-render viewport
                        let viewport = viewport_manager_resize.lock().unwrap();
                        let vp = viewport.get_viewport().clone();
                        viewport.update_task_priorities();
                        drop(viewport);
                        
                        // Get blend parameters from zoom controller
                        let zoom = zoom_controller_wheel.lock().unwrap();
                        let blend_level = zoom.get_next_level();
                        let blend_factor = zoom.get_blend_factor();
                        drop(zoom);
                        
                        let rendered_image = viewport_renderer_wheel.render_viewport(&vp, blend_level, blend_factor);
                        let mut viewport_img = viewport_image_wheel.lock().unwrap();
                        *viewport_img = rendered_image;
                        
                        let width = viewport_img.width() as i32;
                        let height = viewport_img.height() as i32;
                        let raw_data = viewport_img.as_raw().clone();
                        drop(viewport_img);
                        
                        if let Ok(fltk_img) = fltk::image::RgbImage::new(&raw_data, width, height, ColorDepth::Rgba8) {
                            let mut frame = viewport_frame_resize.lock().unwrap();
                            frame.set_image(Some(fltk_img));
                            frame.redraw();
                        }
                        
                        // Calculate render time and time since last render
                        let render_duration = render_start.elapsed().as_secs_f64() * 1000.0;
                        let time_since_last_render = {
                            let mut last_time = last_render_time_wheel.lock().unwrap();
                            let now = Instant::now();
                            let time_since = last_time.map(|t| now.duration_since(t).as_secs_f64() * 1000.0).unwrap_or(0.0);
                            *last_time = Some(now);
                            time_since
                        };
                        
                        // Update last render duration
                        {
                            let mut last_duration = last_render_duration_wheel.lock().unwrap();
                            *last_duration = render_duration;
                        }
                        
                        // Update status bar using unified function
                        if let Ok(status_bar) = status_bar_wheel.lock() {
                            if let Some(mut buf) = status_bar.buffer() {
                                let addr_display = address_display_wheel.lock().unwrap();
                                let address_str = addr_display.get_address();
                                drop(addr_display);
                                
                                let zoom_factor = {
                                    let zoom = zoom_controller_wheel.lock().unwrap();
                                    zoom.get_zoom_factor()
                                };
                                
                                let half_width = (vp.width_pixels as f64) / 2.0;
                                let half_height = (vp.height_pixels as f64) / 2.0;
                                let left = (vp.center_x - half_width).max(0.0) as u64;
                                let right = (vp.center_x + half_width) as u64;
                                let top = (vp.center_y - half_height).max(0.0) as u64;
                                let bottom = (vp.center_y + half_height) as u64;
                                
                                buf.set_text(&format!(
                                    "Address: {}\nRender: {:.1}ms | Since last: {:.1}ms | Zoom: {:.3}x | Level: {} | Viewport: ({}, {}) - ({}, {})", 
                                    address_str,
                                    render_duration,
                                    time_since_last_render,
                                    zoom_factor,
                                    vp.level,
                                    left, top, right, bottom
                                ));
                            }
                        }
                        
                        // Log render times
                        log::info!("Render time: {:.1}ms | Time since last render: {:.1}ms", render_duration, time_since_last_render);
                        
                        win.redraw();
                        true
                    }
                    fltk::enums::Event::KeyDown => {
                        let state = fltk::app::event_state();
                        if state.contains(fltk::enums::EventState::Ctrl) {
                            let key = fltk::app::event_key();
                            let key_char = key.to_char();
                            let is_zero = key_char == Some('0');
                            let is_plus = key_char == Some('+') || key_char == Some('=');
                            let is_minus = key_char == Some('-');

                            if is_zero || is_plus || is_minus {
                                let center_x = (win.width() as f64) / 2.0;
                                let status_bar_height = 60.0;
                                let center_y = ((win.height() as f64) - status_bar_height) / 2.0;
                                let render_start = Instant::now();

                                {
                                    let mut zoom_ctrl = zoom_controller_resize.lock().unwrap();
                                    if is_zero {
                                        log::debug!("Hotkey Ctrl+0: resetting zoom to level 0 (1.0x)");
                                        zoom_ctrl.set_zoom(1.0, center_x, center_y);
                                    } else if is_plus {
                                        log::debug!("Hotkey Ctrl++: zooming in");
                                        zoom_ctrl.zoom_in(center_x, center_y);
                                    } else if is_minus {
                                        log::debug!("Hotkey Ctrl+-: zooming out");
                                        zoom_ctrl.zoom_out(center_x, center_y);
                                    }
                                }

                                // Re-render viewport
                                let viewport = viewport_manager_resize.lock().unwrap();
                                let vp = viewport.get_viewport().clone();
                                viewport.update_task_priorities();
                                drop(viewport);

                                let zoom = zoom_controller_wheel.lock().unwrap();
                                let blend_level = zoom.get_next_level();
                                let blend_factor = zoom.get_blend_factor();
                                drop(zoom);

                                let rendered_image = viewport_renderer_wheel.render_viewport(&vp, blend_level, blend_factor);
                                let mut viewport_img = viewport_image_wheel.lock().unwrap();
                                *viewport_img = rendered_image;

                                let width = viewport_img.width() as i32;
                                let height = viewport_img.height() as i32;
                                let raw_data = viewport_img.as_raw().clone();
                                drop(viewport_img);

                                if let Ok(fltk_img) = fltk::image::RgbImage::new(&raw_data, width, height, ColorDepth::Rgba8) {
                                    let mut frame = viewport_frame_resize.lock().unwrap();
                                    frame.set_image(Some(fltk_img));
                                    frame.redraw();
                                }

                                let render_duration = render_start.elapsed().as_secs_f64() * 1000.0;
                                let time_since_last_render = {
                                    let mut last_time = last_render_time_wheel.lock().unwrap();
                                    let now = Instant::now();
                                    let time_since = last_time.map(|t| now.duration_since(t).as_secs_f64() * 1000.0).unwrap_or(0.0);
                                    *last_time = Some(now);
                                    time_since
                                };

                                if let Ok(status_bar) = status_bar_wheel.lock() {
                                    if let Some(mut buf) = status_bar.buffer() {
                                        let addr_display = address_display_wheel.lock().unwrap();
                                        let address_str = addr_display.get_address();
                                        drop(addr_display);

                                        let zoom_factor = {
                                            let zoom = zoom_controller_wheel.lock().unwrap();
                                            zoom.get_zoom_factor()
                                        };

                                        let half_width = (vp.width_pixels as f64) / 2.0;
                                        let half_height = (vp.height_pixels as f64) / 2.0;
                                        let left = (vp.center_x - half_width).max(0.0) as u64;
                                        let right = (vp.center_x + half_width) as u64;
                                        let top = (vp.center_y - half_height).max(0.0) as u64;
                                        let bottom = (vp.center_y + half_height) as u64;

                                        buf.set_text(&format!(
                                            "Address: {}\nRender: {:.1}ms | Since last: {:.1}ms | Zoom: {:.3}x | Level: {} | Viewport: ({}, {}) - ({}, {})",
                                            address_str, render_duration, time_since_last_render, zoom_factor, vp.level, left, top, right, bottom
                                        ));
                                    }
                                }

                                win.redraw();
                                return true;
                            }
                        }
                        false
                    }
                    fltk::enums::Event::Close => {
                        fltk::app::quit();
                        true
                    }
                    _ => false
                }
            });
        }

        app_window
    }

    /// Handle mouse move event
    pub fn handle_mouse_move(&mut self, x: i32, y: i32) {
        self.mouse_x = x;
        self.mouse_y = y;

        // Update address display
        {
            let viewport = self.viewport_manager.lock().unwrap();
            let vp = viewport.get_viewport();
            let mut addr_display = self.address_display.lock().unwrap();
            addr_display.update_mouse_position(x as u32, y as u32, vp, &self.metadata);
        }

        // Update status bar with current address
        self.update_status_bar();
    }

    /// Handle scroll wheel zoom
    pub fn handle_scroll(&mut self, delta: i32) {
        {
            let mut zoom_ctrl = self.zoom_controller.lock().unwrap();
            let center_x = self.mouse_x as f64;
            let center_y = self.mouse_y as f64;
            if delta > 0 {
                zoom_ctrl.zoom_in(center_x, center_y);
            } else {
                zoom_ctrl.zoom_out(center_x, center_y);
            }
        } // Drop zoom_ctrl lock

        // Update viewport after zoom
        self.update_viewport();
    }

    /// Handle pan drag
    pub fn handle_drag(&mut self, dx: i32, dy: i32) {
        {
            let mut pan_ctrl = self.pan_controller.lock().unwrap();
            pan_ctrl.pan(dx as f64, dy as f64);
        } // Drop pan_ctrl lock

        // Update viewport after pan
        self.update_viewport();
    }

    /// Unified status bar update function with all information
    /// 
    /// This function consolidates all status bar updates to ensure consistent display
    /// across all events (mouse move, drag, zoom, etc.)
    fn update_status_bar(&self) {
        self.update_status_bar_full(None, None);
    }
    
    /// Update status bar with full information including render time
    /// 
    /// Parameters:
    /// - render_time_ms: Optional render duration in milliseconds
    /// - time_since_last_render_ms: Optional time since last render in milliseconds
    fn update_status_bar_full(&self, render_time_ms: Option<f64>, time_since_last_render_ms: Option<f64>) {
        if let Ok(status_bar) = self.status_bar.lock() {
            if let Some(mut buf) = status_bar.buffer() {
                // Get address information
                let addr_display = self.address_display.lock().unwrap();
                let address_str = addr_display.get_address();
                drop(addr_display);
                
                // Get viewport information
                let viewport = self.viewport_manager.lock().unwrap();
                let vp = viewport.get_viewport().clone();
                drop(viewport);
                
                // Get zoom information
                let zoom = self.zoom_controller.lock().unwrap();
                let zoom_factor = zoom.get_zoom_factor();
                drop(zoom);
                
                // Calculate viewport bounds
                let half_width = (vp.width_pixels as f64) / 2.0;
                let half_height = (vp.height_pixels as f64) / 2.0;
                let left = (vp.center_x - half_width).max(0.0) as u64;
                let right = (vp.center_x + half_width) as u64;
                let top = (vp.center_y - half_height).max(0.0) as u64;
                let bottom = (vp.center_y + half_height) as u64;
                
                // Build the status bar text
                let line1 = if address_str == "N/A" {
                    "Address: N/A".to_string()
                } else {
                    format!("Address: {}", address_str)
                };
                
                // Build line 2 with render times if available
                let line2 = if let (Some(render_time), Some(time_since)) = (render_time_ms, time_since_last_render_ms) {
                    format!(
                        "Render: {:.1}ms | Since last: {:.1}ms | Zoom: {:.3}x | Level: {} | Viewport: ({}, {}) - ({}, {})",
                        render_time, time_since, zoom_factor, vp.level, left, top, right, bottom
                    )
                } else {
                    format!(
                        "Zoom: {:.3}x | Level: {} | Viewport: ({}, {}) - ({}, {})",
                        zoom_factor, vp.level, left, top, right, bottom
                    )
                };
                
                // Set the status bar text with both lines
                buf.set_text(&format!("{}\n{}", line1, line2));
                
                // Log the status update if render times are available
                if let (Some(render_time), Some(time_since)) = (render_time_ms, time_since_last_render_ms) {
                    log::info!("Render time: {:.1}ms | Time since last render: {:.1}ms", render_time, time_since);
                }
            }
        }
    }

    /// Load a tile from cache or request from queue
    #[allow(dead_code)]
    fn load_or_request_tile(&self, coord: &TileCoord) -> Option<Vec<u8>> {
        // Try to load from cache first
        match self.cache.load_tile(coord) {
            Ok(tile_data) => {
                log::debug!("Loaded tile from cache: {}", crate::CoordinateParser::pretty_print(*coord));
                Some(tile_data)
            }
            Err(_) => {
                // Tile not in cache, request it from queue
                log::debug!("Tile not in cache, requesting: {}", crate::CoordinateParser::pretty_print(*coord));
                None
            }
        }
    }

    /// Composite tiles into viewport using ViewportRenderer
    fn composite_tiles_into_viewport(&self) {
        let viewport = self.viewport_manager.lock().unwrap();
        let vp = viewport.get_viewport().clone();

        // Use ViewportRenderer to composite tiles with smooth blending
        let zoom = self.zoom_controller.lock().unwrap();
        let blend_level = zoom.get_next_level();
        let blend_factor = zoom.get_blend_factor();
        drop(zoom);
        
        let rendered_image = self.viewport_renderer.render_viewport(&vp, blend_level, blend_factor);
        
        let mut viewport_img = self.viewport_image.lock().unwrap();
        *viewport_img = rendered_image;

        log::debug!(
            "Composited viewport: Level {}, Center ({}, {})",
            vp.level,
            vp.center_x,
            vp.center_y
        );
    }

    /// Draw the viewport image to the FLTK frame
    fn draw_viewport_to_frame(&self) {
        let viewport_img = self.viewport_image.lock().unwrap();
        let width = viewport_img.width() as i32;
        let height = viewport_img.height() as i32;
        
        // Convert RgbaImage to FLTK RgbImage
        let raw_data = viewport_img.as_raw().clone();
        
        if let Ok(fltk_img) = fltk::image::RgbImage::new(&raw_data, width, height, ColorDepth::Rgba8) {
            let mut frame = self.viewport_frame.lock().unwrap();
            frame.set_image(Some(fltk_img));
            frame.redraw();
            
            log::debug!("Drew viewport image to frame: {}x{}", width, height);
        } else {
            log::error!("Failed to create FLTK image from viewport data");
        }
    }

    /// Update viewport rendering
    pub fn update_viewport(&mut self) {
        let viewport = self.viewport_manager.lock().unwrap();
        let vp = viewport.get_viewport().clone();

        // Get visible and adjacent tiles
        let visible_tiles = viewport.get_visible_tiles();
        let adjacent_tiles = viewport.get_adjacent_tiles();

        // Request tiles for visible and adjacent tiles
        viewport.update_task_priorities();

        drop(viewport); // Release lock before compositing

        // Composite tiles into viewport
        self.composite_tiles_into_viewport();
        
        // Draw to FLTK frame
        self.draw_viewport_to_frame();

        // Update address display
        self.update_status_bar();

        log::debug!(
            "Viewport updated: Level {}, Center ({}, {}), Visible tiles: {}, Adjacent tiles: {}",
            vp.level,
            vp.center_x,
            vp.center_y,
            visible_tiles.len(),
            adjacent_tiles.len()
        );
    }

    /// Get current viewport
    pub fn get_viewport(&self) -> Viewport {
        self.viewport_manager
            .lock()
            .unwrap()
            .get_viewport()
            .clone()
    }

    /// Get current address display
    pub fn get_address(&self) -> String {
        self.address_display.lock().unwrap().get_address()
    }

    /// Check if mouse is in bounds
    pub fn is_mouse_in_bounds(&self) -> bool {
        self.address_display.lock().unwrap().is_mouse_in_bounds()
    }

    /// Render the viewport to screen
    pub fn render(&self) {
        self.draw_viewport_to_frame();
    }
}

/// Build the xorviewer URL for a specific location
pub fn build_xorviewer_url(
    metadata: &FileMetadata,
    block: u64,
    page: u64,
    offset_in_page: u64,
) -> Option<String> {
    if metadata.path.is_empty() {
        return None;
    }

    let dump_path = std::path::Path::new(&metadata.path);
    let absolute_path = if dump_path.is_absolute() {
        dump_path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(dump_path)
    };
    let dump_path_str = absolute_path.to_string_lossy();
    let page_start = (block as u64 * metadata.block_size as u64) + page;

    Some(format!(
        "http://localhost/cgi-bin/drresearch/xorviewer.pl?dump={}&pagesize={}&pagesperblock={}&pagestart={}&start={}",
        dump_path_str,
        metadata.page_length,
        metadata.block_size,
        page_start,
        offset_in_page
    ))
}

/// Open the xorviewer URL for a specific location in a web browser
pub fn open_xorviewer_in_browser(
    metadata: &FileMetadata,
    block: u64,
    page: u64,
    offset_in_page: u64,
) -> Option<String> {
    let url = build_xorviewer_url(metadata, block, page, offset_in_page)?;
    log::info!("Opening URL in browser: {}", url);

    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open")
            .arg(&url)
            .spawn();
    }

    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg(&url)
            .spawn();
    }

    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(&["/C", "start", &url])
            .spawn();
    }

    Some(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileLoader;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn create_test_file_loader() -> (NamedTempFile, FileLoader) {
        let mut temp_file = NamedTempFile::new().unwrap();
        // Write 1MB of test data
        let data = vec![0xAA; 1_000_000];
        temp_file.write_all(&data).unwrap();
        temp_file.flush().unwrap();

        let file_loader = FileLoader::new(temp_file.path(), 512, 64).unwrap();
        (temp_file, file_loader)
    }

    #[test]
    #[ignore]
    fn test_app_window_creation() {
        let (_temp_file, file_loader) = create_test_file_loader();
        let metadata = file_loader.get_metadata().clone();
        let task_queue = Arc::new(TaskQueue::new());
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache = Arc::new(
            CacheManager::new(temp_dir.path(), "test.bin".to_string()).unwrap(),
        );

        let window = AppWindow::new(metadata, task_queue, cache, None, None, None);
        assert_eq!(window.metadata.page_length, 512);
        assert_eq!(window.metadata.block_size, 64);
    }

    #[test]
    #[ignore]
    fn test_app_window_mouse_move() {
        let (_temp_file, file_loader) = create_test_file_loader();
        let metadata = file_loader.get_metadata().clone();
        let task_queue = Arc::new(TaskQueue::new());
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache = Arc::new(
            CacheManager::new(temp_dir.path(), "test.bin".to_string()).unwrap(),
        );

        let mut window = AppWindow::new(metadata, task_queue, cache, None, None, None);
        window.handle_mouse_move(100, 100);
        assert_eq!(window.mouse_x, 100);
        assert_eq!(window.mouse_y, 100);
    }

    #[test]
    #[ignore]
    fn test_app_window_initial_viewport() {
        let (_temp_file, file_loader) = create_test_file_loader();
        let metadata = file_loader.get_metadata().clone();
        let task_queue = Arc::new(TaskQueue::new());
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cache = Arc::new(
            CacheManager::new(temp_dir.path(), "test.bin".to_string()).unwrap(),
        );

        let window = AppWindow::new(metadata, task_queue, cache, None, None, None);
        let viewport = window.get_viewport();

        // Should start at level 0 (highest resolution)
        assert_eq!(viewport.level, 0);
        // Should start at upper left corner
        assert_eq!(viewport.center_x, 0.0);
        assert_eq!(viewport.center_y, 0.0);
    }

    #[test]
    fn test_build_xorviewer_url() {
        let meta = FileMetadata::new("dump.bin".to_string(), 100_000, 512, 64);
        let url = build_xorviewer_url(&meta, 2, 5, 128);
        assert!(url.is_some());
        let u = url.unwrap();
        assert!(u.contains("http://localhost/cgi-bin/drresearch/xorviewer.pl"));
        assert!(u.contains("pagesize=512"));
        assert!(u.contains("pagesperblock=64"));
        assert!(u.contains("pagestart=133")); // 2 * 64 + 5 = 133
        assert!(u.contains("start=128"));
    }

    #[test]
    fn test_build_xorviewer_url_empty_path() {
        let meta = FileMetadata::default();
        assert_eq!(build_xorviewer_url(&meta, 0, 0, 0), None);
    }
}
