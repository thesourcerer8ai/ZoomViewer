# Bug Fix: File Dialog Error

## Problem

When running `nand-flash-viewer`, the application immediately failed with:
```
Failed to open file dialog: Invalid metadata: File not found: 
```

## Root Cause

The `main.rs` was calling `file_dialog.open_file("")` with an empty string as the file path. The `FileDialog::open_file()` method expects a valid file path and immediately validates that the file exists. When passed an empty string, it fails validation.

## Solution

Updated `main.rs` to accept the file path in two ways:

### 1. Command-line Argument
```bash
cargo run --release -- /path/to/dump.bin
```

### 2. Interactive Prompt
```bash
cargo run --release
# Then enter path when prompted
```

## Changes Made

**File: `src/main.rs`**

**Before:**
```rust
match file_dialog.open_file("") {
    // ...
}
```

**After:**
```rust
// Get file path from command line or prompt user
let file_path = if let Some(path) = env::args().nth(1) {
    path
} else {
    // Prompt user for file path
    print!("Enter path to NAND dump file: ");
    io::stdout().flush().unwrap();
    
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    input.trim().to_string()
};

if file_path.is_empty() {
    eprintln!("Error: No file path provided");
    eprintln!("Usage: nand-flash-viewer <path-to-dump-file>");
    std::process::exit(1);
}

match file_dialog.open_file(&file_path) {
    // ...
}
```

## Additional Files Created

1. **USAGE.md** - Comprehensive usage guide
2. **create_test_dump.sh** - Script to create test dump files
3. **BUGFIX_FILE_DIALOG.md** - This document

## Testing

To test the fix:

1. Create a test dump file:
   ```bash
   ./create_test_dump.sh test_dump.bin 50
   ```

2. Run the viewer:
   ```bash
   cargo run --release -- test_dump.bin
   ```

3. When prompted, enter:
   - Page length: 2048
   - Block size: 128

The application should now start successfully and begin generating tiles.

## Future Improvements

For a production application, consider:

1. **GUI File Picker**: Integrate with native file dialogs (using `rfd` or `native-dialog` crate)
2. **Recent Files**: Remember recently opened files
3. **Drag & Drop**: Support dragging files onto the window
4. **File Validation**: Better error messages for invalid files
5. **Auto-detection**: Try to detect page length and block size from file structure

## Related Requirements

This fix addresses:
- **Requirement 1.1**: Load NAND Dump Files - now properly accepts file path
- **Requirement 15.1, 15.2**: Accept user-provided parameters - prompts work correctly
- **Requirement 17.3**: Immediate startup - no longer fails before startup
