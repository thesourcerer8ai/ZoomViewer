//! Window geometry persistence using the XDG Base Directory Specification.
//!
//! Per the XDG spec, `$XDG_STATE_HOME` (defaulting to `~/.local/state`) is
//! the right location for "persistent application state data" such as window
//! positions and sizes — as distinguished from user configuration files
//! (XDG_CONFIG_HOME) or caches (XDG_CACHE_HOME).
//!
//! State is stored as JSON in:
//!   `$XDG_STATE_HOME/nand-flash-viewer/window.json`

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The geometry and presentation state of the main window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowState {
    /// Window width in pixels (only meaningful when not fullscreen).
    pub width: i32,
    /// Window height in pixels (only meaningful when not fullscreen).
    pub height: i32,
    /// Window X position on the screen.
    pub x: i32,
    /// Window Y position on the screen.
    pub y: i32,
    /// Whether the window was fullscreen when it was closed.
    pub fullscreen: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        WindowState {
            width: 1024,
            height: 768,
            x: 0,
            y: 0,
            fullscreen: false,
        }
    }
}

/// Manages loading and saving of [`WindowState`] to an XDG-compliant path.
pub struct WindowStateManager {
    /// Resolved path to the JSON state file.
    state_path: PathBuf,
}

impl WindowStateManager {
    /// Create a manager that resolves the XDG state directory automatically.
    ///
    /// Resolution order for the base directory:
    /// 1. `$XDG_STATE_HOME`
    /// 2. `$HOME/.local/state`
    /// 3. Falls back to the current directory if neither is set.
    pub fn new() -> Self {
        let base = std::env::var("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::var("HOME")
                    .map(|h| PathBuf::from(h).join(".local").join("state"))
                    .unwrap_or_else(|_| PathBuf::from("."))
            });

        let state_path = base.join("nand-flash-viewer").join("window.json");
        WindowStateManager { state_path }
    }

    /// Load the persisted window state.
    ///
    /// Returns `Ok(WindowState::default())` if no state has been saved yet.
    /// Returns an error only if the file exists but cannot be parsed.
    pub fn load(&self) -> Result<WindowState, String> {
        if !self.state_path.exists() {
            return Ok(WindowState::default());
        }

        let content = std::fs::read_to_string(&self.state_path)
            .map_err(|e| format!("Failed to read window state file: {}", e))?;

        serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse window state: {}", e))
    }

    /// Persist the given window state to disk.
    ///
    /// Creates parent directories as needed. Errors are logged but not fatal
    /// so a save failure never crashes the application on exit.
    pub fn save(&self, state: &WindowState) -> Result<(), String> {
        // Ensure the directory exists.
        if let Some(parent) = self.state_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create state directory: {}", e))?;
        }

        let json = serde_json::to_string_pretty(state)
            .map_err(|e| format!("Failed to serialise window state: {}", e))?;

        std::fs::write(&self.state_path, json)
            .map_err(|e| format!("Failed to write window state file: {}", e))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn manager_in(dir: &TempDir) -> WindowStateManager {
        WindowStateManager {
            state_path: dir.path().join("nand-flash-viewer").join("window.json"),
        }
    }

    #[test]
    fn default_when_no_file() {
        let dir = TempDir::new().unwrap();
        let mgr = manager_in(&dir);
        let state = mgr.load().unwrap();
        assert_eq!(state, WindowState::default());
    }

    #[test]
    fn round_trip() {
        let dir = TempDir::new().unwrap();
        let mgr = manager_in(&dir);

        let original = WindowState {
            width: 1920,
            height: 1080,
            x: 100,
            y: 200,
            fullscreen: false,
        };

        mgr.save(&original).unwrap();
        let loaded = mgr.load().unwrap();
        assert_eq!(loaded, original);
    }

    #[test]
    fn round_trip_fullscreen() {
        let dir = TempDir::new().unwrap();
        let mgr = manager_in(&dir);

        let original = WindowState {
            width: 1920,
            height: 1080,
            x: 0,
            y: 0,
            fullscreen: true,
        };

        mgr.save(&original).unwrap();
        let loaded = mgr.load().unwrap();
        assert_eq!(loaded, original);
    }

    #[test]
    fn creates_parent_directories() {
        let dir = TempDir::new().unwrap();
        let mgr = manager_in(&dir);
        // Directory doesn't exist yet — save should create it.
        assert!(!dir.path().join("nand-flash-viewer").exists());
        mgr.save(&WindowState::default()).unwrap();
        assert!(dir.path().join("nand-flash-viewer").join("window.json").exists());
    }
}
