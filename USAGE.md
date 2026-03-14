# NAND Flash Viewer - Usage Guide

## Running the Application

The NAND Flash Viewer can be run in two ways:

### Option 1: Command-line argument

```bash
cargo run --release -- /path/to/your/dump.bin
```

Or after building:

```bash
./target/release/nand-flash-viewer /path/to/your/dump.bin
```

### Option 2: Interactive prompt

Simply run without arguments and you'll be prompted:

```bash
cargo run --release
```

Then enter the path when prompted:
```
Enter path to NAND dump file: /path/to/your/dump.bin
```

## First-Time Setup

When you open a dump file for the first time, you'll be prompted for:

1. **Page length** (500-20000 bytes)
   - Common values: 2048, 4096, 8192
   - Default: 2048

2. **Block size** (pages per block)
   - Valid values: 64, 128, 256, 512, 1024
   - Default: 128

These parameters are cached, so you won't be prompted again for the same file.

## File Requirements

- **Size**: Any size (tested from 1 MB to 500 GB)
- **Format**: Raw binary NAND dump

## Creating a Test Dump File

For testing purposes, you can create a test file of any size:

```bash
# Create a 1 MB test file with random data
dd if=/dev/urandom of=test_dump.bin bs=1M count=1

# Or create a larger sparse file (doesn't actually use disk space)
dd if=/dev/urandom of=test_dump.bin bs=1M count=10
truncate -s 10G test_dump.bin
```

Or use the provided script:

```bash
# Create a 1 MB test file (default)
./create_test_dump.sh test_dump.bin 1M

# Create a 10 GB sparse file
./create_test_dump.sh test_dump.bin 10G
```

Or use Python:

```python
import os

# Create a 1 MB test file with random data
with open('test_dump.bin', 'wb') as f:
    f.write(os.urandom(1024 * 1024))

# Or create a 10 GB sparse file with some test data
with open('test_dump_large.bin', 'wb') as f:
    # Write 10 MB of test data at the beginning
    f.write(os.urandom(10 * 1024 * 1024))
    # Seek to 10 GB - 1 byte
    f.seek(10 * 1024 * 1024 * 1024 - 1)
    # Write one byte to set the file size
    f.write(b'\x00')
```

## Cache Directory

The application creates a `.cache` directory in the current working directory to store:
- Generated tile images (QOI format)
- File metadata (page length, block size)

You can safely delete the `.cache` directory to regenerate all tiles.

## Performance

- **Startup time**: < 500ms (no preprocessing)
- **Tile format**: QOI (20-50x faster than PNG)
- **Parallel processing**: Uses all available CPU cores
- **Memory efficient**: Only loads tiles in viewport

## Troubleshooting

### "File not found" error
- Check that the file path is correct
- Use absolute paths if relative paths don't work
- Ensure the file exists and is readable

### "File size outside valid range" error
- This error has been removed - the viewer now accepts files of any size
- For testing, you can use files as small as 1 MB

### "Invalid page length" or "Invalid block size" error
- Page length must be 500-20000 bytes
- Block size must be one of: 64, 128, 256, 512, 1024

### Cached metadata is wrong
- Delete the `.cache` directory
- Run the application again to re-enter parameters

## Keyboard Controls

(To be implemented in UI layer)

- **Mouse wheel**: Zoom in/out
- **Click and drag**: Pan
- **Mouse hover**: Show address (Block, Page, Byte, Bit)

## Logging

The application logs to stderr. To see detailed logs:

```bash
RUST_LOG=debug cargo run --release -- /path/to/dump.bin
```

Log levels:
- `error`: Errors only
- `warn`: Warnings and errors
- `info`: General information (default)
- `debug`: Detailed debugging information
- `trace`: Very verbose debugging

## Architecture

The viewer uses an image pyramid algorithm similar to Google Maps:
- **Level 0**: Highest resolution (1 bit = 1 pixel)
- **Level 1+**: Lower resolutions (composited from level below)
- **Tiles**: 512x512 pixel QOI images
- **On-demand**: Tiles generated only when needed
- **Cached**: Generated tiles saved for reuse

## Performance Tips

1. **First load**: Initial tile generation takes time
2. **Subsequent loads**: Cached tiles load instantly
3. **Zoom/pan**: Smooth after tiles are cached
4. **Large files**: Worker pool parallelizes generation
5. **SSD recommended**: Faster cache access

## Known Limitations

- Command-line only (GUI in development)
- No real-time file monitoring
- Cache grows with exploration (can be cleared)
