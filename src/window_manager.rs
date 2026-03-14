//! Window/tab management for multi-file support
//!
//! Manages multiple application windows or tabs, each displaying a different dump file.
//! Coordinates window lifecycle and state management.

use crate::multi_file_manager::DumpId;
use std::collections::HashMap;

/// Unique identifier for a window/tab
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(u64);

impl WindowId {
    /// Create a new window ID
    pub fn new(id: u64) -> Self {
        WindowId(id)
    }
}

/// Window/tab state
pub struct WindowState {
    /// Unique identifier for this window
    pub id: WindowId,
    /// Associated dump file ID
    pub dump_id: DumpId,
    /// Window title
    pub title: String,
    /// Whether window is active/focused
    pub is_active: bool,
}

/// Window manager for multi-file support
pub struct WindowManager {
    /// Map of window IDs to their state
    windows: HashMap<WindowId, WindowState>,
    /// Next window ID to assign
    next_id: u64,
    /// Currently active window
    active_window: Option<WindowId>,
}

impl WindowManager {
    /// Create a new window manager
    pub fn new() -> Self {
        WindowManager {
            windows: HashMap::new(),
            next_id: 1,
            active_window: None,
        }
    }

    /// Open a new window for a dump file
    ///
    /// # Arguments
    /// * `dump_id` - The dump file to display in this window
    /// * `title` - Window title
    ///
    /// # Returns
    /// The WindowId for the newly created window
    pub fn open_window(&mut self, dump_id: DumpId, title: String) -> WindowId {
        let id = WindowId::new(self.next_id);
        self.next_id += 1;

        let state = WindowState {
            id,
            dump_id,
            title,
            is_active: true,
        };

        // If this is the first window, make it active
        if self.active_window.is_none() {
            self.active_window = Some(id);
        }

        self.windows.insert(id, state);

        log::info!("Opened window: id={:?}, dump_id={:?}", id, dump_id);

        id
    }

    /// Close a window
    pub fn close_window(&mut self, id: WindowId) -> bool {
        if self.windows.remove(&id).is_some() {
            // If this was the active window, switch to another
            if self.active_window == Some(id) {
                self.active_window = self.windows.keys().next().copied();
            }

            log::info!("Closed window: id={:?}", id);
            true
        } else {
            false
        }
    }

    /// Get the state for a specific window
    pub fn get_window(&self, id: WindowId) -> Option<&WindowState> {
        self.windows.get(&id)
    }

    /// Get mutable state for a specific window
    pub fn get_window_mut(&mut self, id: WindowId) -> Option<&mut WindowState> {
        self.windows.get_mut(&id)
    }

    /// Set the active window
    pub fn set_active_window(&mut self, id: WindowId) -> bool {
        if self.windows.contains_key(&id) {
            // Deactivate previous active window
            if let Some(prev_id) = self.active_window {
                if let Some(prev_window) = self.windows.get_mut(&prev_id) {
                    prev_window.is_active = false;
                }
            }

            // Activate new window
            if let Some(window) = self.windows.get_mut(&id) {
                window.is_active = true;
            }

            self.active_window = Some(id);
            log::info!("Set active window: id={:?}", id);
            true
        } else {
            false
        }
    }

    /// Get the currently active window
    pub fn get_active_window(&self) -> Option<WindowId> {
        self.active_window
    }

    /// Get list of all open window IDs
    pub fn list_windows(&self) -> Vec<WindowId> {
        self.windows.keys().copied().collect()
    }

    /// Get the number of open windows
    pub fn window_count(&self) -> usize {
        self.windows.len()
    }

    /// Update window title
    pub fn set_window_title(&mut self, id: WindowId, title: String) -> bool {
        if let Some(window) = self.windows.get_mut(&id) {
            window.title = title;
            true
        } else {
            false
        }
    }
}

impl Default for WindowManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_window_manager_creation() {
        let manager = WindowManager::new();
        assert_eq!(manager.window_count(), 0);
        assert_eq!(manager.get_active_window(), None);
    }

    #[test]
    fn test_open_window() {
        let mut manager = WindowManager::new();
        let dump_id = DumpId::new(1);

        let window_id = manager.open_window(dump_id, "Test Window".to_string());

        assert_eq!(manager.window_count(), 1);
        assert_eq!(manager.get_active_window(), Some(window_id));

        let window = manager.get_window(window_id).unwrap();
        assert_eq!(window.dump_id, dump_id);
        assert_eq!(window.title, "Test Window");
        assert!(window.is_active);
    }

    #[test]
    fn test_close_window() {
        let mut manager = WindowManager::new();
        let dump_id = DumpId::new(1);

        let window_id = manager.open_window(dump_id, "Test Window".to_string());
        assert_eq!(manager.window_count(), 1);

        let result = manager.close_window(window_id);
        assert!(result);
        assert_eq!(manager.window_count(), 0);
        assert_eq!(manager.get_active_window(), None);
    }

    #[test]
    fn test_multiple_windows() {
        let mut manager = WindowManager::new();

        let dump_id1 = DumpId::new(1);
        let dump_id2 = DumpId::new(2);

        let window_id1 = manager.open_window(dump_id1, "Window 1".to_string());
        let window_id2 = manager.open_window(dump_id2, "Window 2".to_string());

        assert_eq!(manager.window_count(), 2);
        assert_ne!(window_id1, window_id2);

        let windows = manager.list_windows();
        assert_eq!(windows.len(), 2);
        assert!(windows.contains(&window_id1));
        assert!(windows.contains(&window_id2));
    }

    #[test]
    fn test_set_active_window() {
        let mut manager = WindowManager::new();

        let dump_id1 = DumpId::new(1);
        let dump_id2 = DumpId::new(2);

        let window_id1 = manager.open_window(dump_id1, "Window 1".to_string());
        let window_id2 = manager.open_window(dump_id2, "Window 2".to_string());

        // First window should be active
        assert_eq!(manager.get_active_window(), Some(window_id1));

        // Switch to second window
        let result = manager.set_active_window(window_id2);
        assert!(result);
        assert_eq!(manager.get_active_window(), Some(window_id2));

        // Verify first window is no longer active
        let window1 = manager.get_window(window_id1).unwrap();
        assert!(!window1.is_active);

        // Verify second window is active
        let window2 = manager.get_window(window_id2).unwrap();
        assert!(window2.is_active);
    }

    #[test]
    fn test_close_active_window_switches_to_another() {
        let mut manager = WindowManager::new();

        let dump_id1 = DumpId::new(1);
        let dump_id2 = DumpId::new(2);

        let window_id1 = manager.open_window(dump_id1, "Window 1".to_string());
        let window_id2 = manager.open_window(dump_id2, "Window 2".to_string());

        // First window is active
        assert_eq!(manager.get_active_window(), Some(window_id1));

        // Close first window
        manager.close_window(window_id1);

        // Second window should now be active
        assert_eq!(manager.get_active_window(), Some(window_id2));
    }

    #[test]
    fn test_set_window_title() {
        let mut manager = WindowManager::new();
        let dump_id = DumpId::new(1);

        let window_id = manager.open_window(dump_id, "Original Title".to_string());

        let result = manager.set_window_title(window_id, "New Title".to_string());
        assert!(result);

        let window = manager.get_window(window_id).unwrap();
        assert_eq!(window.title, "New Title");
    }

    #[test]
    fn test_get_window_mut() {
        let mut manager = WindowManager::new();
        let dump_id = DumpId::new(1);

        let window_id = manager.open_window(dump_id, "Test".to_string());

        if let Some(window) = manager.get_window_mut(window_id) {
            window.title = "Modified".to_string();
        }

        let window = manager.get_window(window_id).unwrap();
        assert_eq!(window.title, "Modified");
    }
}
