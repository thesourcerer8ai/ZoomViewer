//! Logging infrastructure for the NAND Flash Viewer

use log::LevelFilter;

/// Initialize the logging system
///
/// Sets up env_logger with a default level of INFO if not already configured.
/// Can be overridden with the RUST_LOG environment variable.
pub fn init() {
    let _ = env_logger::builder()
        .filter_level(LevelFilter::Debug) // Changed from Info to Debug to see more output
        .parse_default_env() // Explicitly parse RUST_LOG environment variable
        .try_init();
}

/// Initialize logging with a specific level
pub fn init_with_level(level: LevelFilter) {
    let _ = env_logger::builder()
        .filter_level(level)
        .try_init();
}
