# Tile Rendering Bug Fix

## Problem
Tiles were displaying incorrectly - only the top pixel line showed data, while the rest of the tile was white.

## Root Cause Analysis

### Issue 1: Incorrect Byte-to-Pixel Mapping in render_tile_data()
The `render_tile_data()` method was iterating through each pixel and reading one byte per pixel, but each byte should render 8 horizontal pixels (one per bit).

### Issue 2: Incorrect Fragment Calculation
The `calculate_fragments()` method had a fundamental misunderstanding of the NAND flash data layout:

**Incorrect understanding:**
- Assumed bytes within a row were at different file offsets based on their X position
- Calculated one offset per (byte_x, byte_y) position
- This resulted in all bytes in a row mapping to the same offset (e.g., row 0 bytes all mapped to offset 0)

**Correct understanding (from requirements):**
- Pages are stored as consecutive bytes in the file
- Page N starts at offset `N * page_length`
- Within a page, bytes are arranged horizontally (byte 0 is leftmost, byte 511 is rightmost)
- Pages represent horizontal rows of data (page 0 = row 0, page 1 = row 1, etc.)
- For a tile needing rows 0-255 and columns 0-31:
  - Row 0: Page 0, bytes 0-31 (file offset 0-31)
  - Row 1: Page 1, bytes 0-31 (file offset 512-543)
  - Row 2: Page 2, bytes 0-31 (file offset 1024-1055)
  - etc.

## Solution

### Fix 1: render_tile_data()
Rewrote to iterate through bytes and render 8 pixels per byte:
1. Iterate through rows (y-axis)
2. For each row, iterate through bytes (32 bytes per row for 256-pixel width)
3. For each byte, render 8 pixels horizontally using bit extraction
4. Handle edge cases where tile_data runs out before filling the entire tile

### Fix 2: calculate_fragments()
Completely rewrote to correctly calculate byte ranges:
1. For each row in the tile (e.g., rows 0-255)
2. Calculate which page that row belongs to (row N = page N)
3. Calculate byte offset: `page_number * page_length + start_byte_x`
4. Create fragment for that row's byte range
5. Add bounds checking to prevent reading beyond file size

## Changes Made

### File: `src/tile_generator.rs`

**Method: `calculate_fragments()`**
- Removed complex block/grid calculations that were incorrect
- Simplified to row-based fragment calculation
- Each row generates one fragment: `[page_num * page_length + start_x, page_num * page_length + end_x)`
- Added bounds checking to prevent reading beyond file size

**Method: `render_tile_data()`**
- Removed complex coordinate calculations
- Simplified to iterate through bytes and render 8 pixels per byte
- Added proper handling for incomplete data (fill remaining pixels with white)
- Fixed bit extraction to use MSB-first ordering (bit 7 = leftmost pixel)

**Test Updates:**
- Updated all QOI magic number assertions from PNG-style `b"\x89QOI\r\n\x1a\n"` to QOI format `b"qoif"`
- Simplified `test_qoi_output_structure()` to check for QOI header structure

## Testing
All tests pass:
- 39 unit tests passed
- 6 property-based tests ignored (as configured)
- Compilation successful with no errors

## Expected Result
Tiles should now display correctly with all pixels rendered from the dump data across all rows, not just the top line.

## Technical Details

### Fragment Structure for Tile (0,0)
For a 256x256 pixel tile (32 bytes wide, 256 rows tall) with page_length=512:
- 256 fragments (one per row)
- Fragment 0: bytes 0-31 (page 0, columns 0-31)
- Fragment 1: bytes 512-543 (page 1, columns 0-31)
- Fragment 2: bytes 1024-1055 (page 2, columns 0-31)
- ...
- Fragment 255: bytes 130560-130591 (page 255, columns 0-31)

These fragments are NOT consecutive in the file, which is why we need multiple fragments rather than one large contiguous read.
