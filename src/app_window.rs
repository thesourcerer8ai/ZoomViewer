//! Main application window with viewport rendering and event handling

use crate::{
    AddressDisplay, CacheManager, FileMetadata, PanController, TaskQueue, TileCoord, Viewport,
    ViewportManager, ViewportRenderer, ZoomController,
};
use fltk::{prelude::*, text::TextEditor, window::Window, frame::Frame, enums::{ColorDepth, Event}, app::MouseWheel, image::RgbImage};
use image::RgbaImage;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::Receiver;
use std::time::Instant;
use std::collections::HashMap;

const TILE_WIDTH: u32 = 256;
const TILE_HEIGHT: u32 = 256;

/// Main application window
#[allow(dead_code)]
pub struct AppWindow {
    /// FLTK window
    window: Window,
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

impl AppWindow {
    /// Create a new application window
    pub fn new(
        metadata: FileMetadata,
        task_queue: Arc<TaskQueue>,
        cache: Arc<CacheManager>,
        tile_rx: Option<Receiver<TileCoord>>,
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
        
        window.make_resizable(true);
        window.size_range(800, 600, 0, 0); // Min size 800x600, no max size

        // Create viewport frame to display the image
        let mut viewport_frame = Frame::default()
            .with_size(1024, 708)
            .with_pos(0, 0);
        
        // Make the frame resizable
        viewport_frame.set_frame(fltk::enums::FrameType::FlatBox);

        // Create viewport manager
        let viewport_manager = Arc::new(Mutex::new(ViewportManager::new(
            metadata.clone(),
            task_queue.clone(),
        )));

        // Create zoom controller
        let zoom_controller = Arc::new(Mutex::new(ZoomController::new(
            metadata.clone(),
            viewport_manager.clone(),
            1024,
            708, // Account for status bar height
        )));

        // Create pan controller
        let pan_controller = Arc::new(Mutex::new(PanController::new(
            metadata.clone(),
            viewport_manager.clone(),
            1024,
            708,
        )));

        // Create address display
        let address_display = Arc::new(Mutex::new(AddressDisplay::new()));

        // Create viewport renderer
        let viewport_renderer = Arc::new(ViewportRenderer::new(cache.clone(), TILE_WIDTH, TILE_HEIGHT));

        // Initialize viewport at upper left corner with default zoom (level 0)
        // Center the viewport so that (0,0) is at the upper left corner
        {
            let mut vm = viewport_manager.lock().unwrap();
            let center_x = 1024.0 / 2.0; // Half of viewport width
            let center_y = 708.0 / 2.0;  // Half of viewport height
            vm.update_viewport(0, center_x, center_y, 1024, 708);
            vm.update_task_priorities();
        }

        // Create status bar for address display
        let mut status_bar = TextEditor::default()
            .with_size(1024, 60)
            .with_pos(0, 708);
        status_bar.set_buffer(fltk::text::TextBuffer::default());
        // Note: TextEditor doesn't have set_editable, but we can make it read-only by not allowing input

        window.end();
        window.show();

        // Create initial viewport image
        let viewport_image = Arc::new(Mutex::new(RgbaImage::new(1024, 708)));
        
        // Create FLTK tile cache
        let fltk_tile_cache = Arc::new(Mutex::new(HashMap::new()));

        let status_bar_arc = Arc::new(Mutex::new(status_bar));
        let viewport_frame_arc = Arc::new(Mutex::new(viewport_frame));
        
        // Clone metadata for event handlers before moving into struct
        let metadata_for_events = metadata.clone();

        let app_window = AppWindow {
            window,
            viewport_frame: viewport_frame_arc.clone(),
            viewport_manager: viewport_manager.clone(),
            zoom_controller: zoom_controller.clone(),
            pan_controller: pan_controller.clone(),
            address_display,
            metadata,
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
                            
                            // Convert dump file path to absolute path
                            let dump_path = std::path::Path::new(&metadata.path);
                            let absolute_path = if dump_path.is_absolute() {
                                dump_path.to_path_buf()
                            } else {
                                std::env::current_dir()
                                    .unwrap_or_else(|_| std::path::PathBuf::from("."))
                                    .join(dump_path)
                            };
                            let dump_path_str = absolute_path.to_string_lossy();
                            
                            // Calculate page start: block * block_size + page_number
                            let page_start = (block as u32 * metadata.block_size) + (page as u32);
                            
                            // Build the URL
                            let url = format!(
                                "http://localhost/cgi-bin/drresearch/xorviewer.pl?dump={}&pagesize={}&pagesperblock={}&pagestart={}&start={}",
                                dump_path_str,
                                metadata.page_length,
                                metadata.block_size,
                                page_start,
                                byte
                            );
                            
                            log::info!("Opening URL: {}", url);
                            
                            // Open URL in browser
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
        
        // Set up tile completion listener if receiver is available
        if let Some(rx) = tile_rx {
            let viewport_image_clone = app_window.viewport_image.clone();
            let viewport_frame_clone = app_window.viewport_frame.clone();
            let viewport_renderer_clone = app_window.viewport_renderer.clone();
            let viewport_manager_clone = app_window.viewport_manager.clone();
            let zoom_controller_clone = app_window.zoom_controller.clone();
            
            // Use FLTK's channel mechanism to receive notifications from worker threads
            let (s, r) = fltk::app::channel::<TileCoord>();
            
            // Spawn a thread to bridge between std::sync::mpsc and fltk::app::channel
            std::thread::spawn(move || {
                while let Ok(coord) = rx.recv() {
                    let _ = s.send(coord);
                }
            });
            
            // Set up FLTK receiver callback using add_timeout
            fltk::app::add_timeout3(0.1, move |handle| {
                // Check for tile completion messages
                if let Some(coord) = r.recv() {
                    log::debug!("Tile completed notification received: {:?}", coord);
                    
                    // Re-render viewport
                    let viewport = viewport_manager_clone.lock().unwrap();
                    let vp = viewport.get_viewport().clone();
                    drop(viewport);
                    
                    // Get blend parameters from zoom controller
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
                fltk::app::repeat_timeout3(0.1, handle);
            });
        }

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

        let window = AppWindow::new(metadata, task_queue, cache, None);
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

        let mut window = AppWindow::new(metadata, task_queue, cache, None);
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

        let window = AppWindow::new(metadata, task_queue, cache, None);
        let viewport = window.get_viewport();

        // Should start at level 0 (highest resolution)
        assert_eq!(viewport.level, 0);
        // Should start at upper left corner
        assert_eq!(viewport.center_x, 0.0);
        assert_eq!(viewport.center_y, 0.0);
    }
}
