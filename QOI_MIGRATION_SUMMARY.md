# QOI Migration Summary

## Overview

Successfully migrated the NAND Flash Viewer from PNG to QOI (Quite OK Image Format) for tile caching.

## Changes Made

### 1. Dependencies (Cargo.toml)
- **Removed**: `png = "0.16"`
- **Added**: `qoi = "0.4"`

### 2. Core Modules Updated

#### cache_manager.rs
- Updated file extension from `.png` to `.qoi`
- Changed signature validation from PNG magic bytes to QOI magic bytes (`qoif`)
- Updated all comments and documentation

#### tile_generator.rs
- Replaced `encode_png()` with `encode_qoi()`
- Converted from RGB (3 bytes/pixel) to RGBA (4 bytes/pixel) format
- Simplified encoding using `qoi::encode_to_vec()`
- Updated all tests to use QOI decoding API

#### pyramid_tile_generator.rs
- Replaced `encode_png()` with `encode_qoi()`
- Replaced `decode_png()` with `decode_qoi()`
- Converted from RGB to RGBA format
- Updated all tests and comments

#### error.rs
- Added `ImageError` variant for generic image encoding/decoding errors
- Kept `PngError` for backward compatibility

### 3. API Differences

**PNG API (old)**:
```rust
let mut encoder = png::Encoder::new(&mut png_data, width, height);
encoder.set_color(png::ColorType::RGB);
encoder.set_depth(png::BitDepth::Eight);
let mut writer = encoder.write_header()?;
writer.write_image_data(&rgb_data)?;
```

**QOI API (new)**:
```rust
let qoi_data = qoi::encode_to_vec(&rgba_data, width, height)?;
```

**Decoding**:
- PNG: Multi-step process with Decoder, reader, buffer
- QOI: Single function call `qoi::decode_to_vec()`

### 4. Format Changes

| Aspect | PNG | QOI |
|--------|-----|-----|
| Channels | RGB (3 bytes/pixel) | RGBA (4 bytes/pixel) |
| Signature | 8 bytes (`\x89PNG\r\n\x1a\n`) | 4 bytes (`qoif`) |
| Header Size | Variable | 14 bytes fixed |
| API Complexity | High (multi-step) | Low (single function) |

## Benefits

1. **Speed**: QOI encoding/decoding is 20-50x faster than PNG
2. **Simplicity**: Much simpler API, less code
3. **Deterministic**: No compression levels, consistent performance
4. **Perfect for binary data**: Excellent for black/white pixel patterns

## Testing

All tests pass successfully:
- ✅ Cache save/load operations
- ✅ Tile generation and encoding
- ✅ Pyramid tile composition
- ✅ Round-trip encoding/decoding
- ✅ File format validation

## Performance Impact

Expected improvements:
- **Tile generation**: 20-50x faster encoding
- **Cache loading**: 20-50x faster decoding
- **Worker throughput**: Significantly higher tiles/second
- **Memory usage**: Slightly lower during encoding/decoding

## File Size Impact

QOI files may be slightly larger than PNG for highly compressible data (like solid black/white regions), but:
- The speed gain far outweighs the size difference
- Disk space is cheap, CPU time is expensive
- For binary visualization patterns, QOI is still very efficient

## Backward Compatibility

- Old PNG cache files will not be recognized (different signature)
- Cache will be regenerated automatically on first run
- No user intervention required

## Conclusion

The migration to QOI is complete and successful. The system now benefits from significantly faster tile generation and caching, which directly improves UI responsiveness and overall performance.
