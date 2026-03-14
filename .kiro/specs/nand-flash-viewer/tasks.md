# Implementation Plan: NAND Flash Viewer

## Overview

This implementation plan breaks down the NAND Flash Viewer into discrete Rust coding tasks, organized by dependency order. The system will be built incrementally, starting with core data structures and utilities, progressing through file/cache management, tile generation, worker management, and finally UI integration. Each task builds on previous work, with property-based tests validating correctness properties throughout.

## Tasks

- [x] 1. Set up project structure and core data types
  - Create Rust project with Cargo.toml dependencies (image, png, rayon, parking_lot, serde, serde_json)
  - Define core data structures: FileMetadata, TileCoord, PyramidLevel, TileTask, Viewport, Fragment
  - Define error types and Result wrapper
  - Set up logging infrastructure
  - _Requirements: 1.1, 1.2, 1.3, 1.6_

- [x] 2. Implement CoordinateParser for bidirectional tile ↔ byte offset conversion
  - [x] 2.1 Implement tile-to-byte-offset conversion algorithm
    - Account for page length, block size, and grid layout
    - Handle level-based scaling (2^level multiplier)
    - _Requirements: 18.1, 18.2, 18.3_
  
  - [x] 2.2 Write property test for coordinate round-trip
    - **Property 46: Coordinate round-trip**
    - **Validates: Requirements 18.4**
  
  - [x] 2.3 Implement byte-offset-to-tile conversion (reverse algorithm)
    - Reverse the tile-to-byte-offset calculation
    - _Requirements: 18.1, 18.2, 18.3_
  
  - [x] 2.4 Write unit tests for coordinate conversion edge cases
    - Test boundary conditions (first/last tile, various page/block sizes)
    - Test round-trip with random coordinates
    - _Requirements: 18.1, 18.4_
  
  - [x] 2.5 Implement pretty-printer for tile coordinates
    - Format as "L{level}:({x},{y})" for logging
    - _Requirements: 18.5_

- [x] 3. Implement BitRenderer for bit-to-pixel rendering
  - [x] 3.1 Create Pixel type and PixelBuffer abstraction
    - Define Pixel as RGB tuple or struct
    - Implement PixelBuffer for 2D pixel array
    - _Requirements: 2.1, 2.2, 2.3_
  
  - [x] 3.2 Implement renderBit function (1→black, 0→white)
    - Render single bit as pixel
    - _Requirements: 2.1, 2.2, 2.3_
  
  - [x] 3.5 Write property test for bit rendering
    - **Property 7: Bit-to-pixel rendering**
    - **Validates: Requirements 2.1, 2.2, 2.3, 2.4, 2.5**
  
  - [x] 3.3 Implement renderByte function (MSB-left, LSB-right)
    - Render 8 bits horizontally with correct bit ordering
    - _Requirements: 2.4, 2.5_
  
  - [x] 3.4 Write unit tests for bit rendering edge cases
    - Test all byte values (0x00, 0xFF, 0xAA, 0x55)
    - Test bit ordering correctness
    - _Requirements: 2.1, 2.4, 2.5_

- [x] 4. Implement ByteArranger for horizontal byte layout
  - [x] 4.1 Implement calculatePageWidth function
    - Return pageLength * 8 (pixels, no spacing)
    - _Requirements: 3.1, 3.2, 3.3, 3.4_
  
  - [x] 4.2 Implement renderPage function
    - Render bytes left-to-right with no spacing
    - Use BitRenderer for each byte
    - _Requirements: 3.1, 3.2, 3.3, 3.4_
  
  - [x] 4.3 Write property test for byte arrangement
    - **Property 8: Byte horizontal arrangement**
    - **Validates: Requirements 3.1, 3.2, 3.3, 3.4**
  
  - [x] 4.4 Write unit tests for byte arrangement edge cases
    - Test single byte, multiple bytes, full page
    - Test pixel positioning accuracy
    - _Requirements: 3.1, 3.4_

- [x] 5. Implement BlockArranger for vertical page/block layout
  - [x] 5.1 Implement calculateBlockHeight function
    - Return blockSize * pageLength * 8 + (blockSize-1) * pageSpacing
    - _Requirements: 4.1, 4.2, 4.3, 4.4_
  
  - [x] 5.2 Implement calculateGridDimensions function (4:3 aspect ratio)
    - Calculate gridWidth and gridHeight for 4:3 layout
    - Use algorithm from design: solve width*height=totalPixels with width/height=4/3
    - _Requirements: 4.6, 4.7_
  
  - [x] 5.3 Write property test for grid layout aspect ratio
    - **Property 11: Grid layout arrangement**
    - **Validates: Requirements 4.6, 4.7**
  
  - [x] 5.4 Implement renderBlock function
    - Render block at grid position with page/block spacing
    - Use ByteArranger for each page
    - _Requirements: 4.1, 4.2, 4.3, 4.4, 4.5_
  
  - [x] 5.5 Write property test for block spacing hierarchy
    - **Property 10: Block spacing hierarchy**
    - **Validates: Requirements 4.5**
  
  - [x] 5.6 Write unit tests for block arrangement edge cases
    - Test single block, multiple blocks, grid layout
    - Test spacing calculations
    - _Requirements: 4.1, 4.5, 4.6_

- [x] 6. Implement FileLoader for dump file access
  - [x] 6.1 Implement FileLoader::new with file opening
    - Open dump file in read-only mode
    - Detect or accept metadata (page length, block size)
    - Calculate derived metadata (totalPages, totalBlocks, gridWidth, gridHeight)
    - _Requirements: 1.1, 1.2, 1.3, 1.6_
  
  - [x] 6.2 Implement readBytes function
    - Read bytes from dump file at offset
    - Handle I/O errors gracefully
    - _Requirements: 1.1_
  
  - [x] 6.3 Implement readFragments function
    - Read multiple fragments and concatenate
    - Optimize for contiguous fragments
    - _Requirements: 6.1, 6.2_
  
  - [x] 6.4 Write unit tests for file loading
    - Test reading various byte ranges
    - Test error handling (file not found, permission denied)
    - _Requirements: 1.1_

- [x] 7. Implement CacheManager for hierarchical cache directory structure
  - [x] 7.1 Implement CacheManager::new
    - Create .cache directory if needed
    - _Requirements: 8.1, 19.1_
  
  - [x] 7.2 Implement getTilePath function
    - Return .cache/{dump_filename}/{level}/{block_y}/{block_x}.png
    - _Requirements: 8.2, 19.2, 19.3_
  
  - [x] 7.3 Implement tileExists function
    - Check if tile file exists in cache
    - _Requirements: 8.3_
  
  - [x] 7.4 Implement loadTile function
    - Load PNG from cache
    - Validate PNG integrity
    - _Requirements: 8.4_
  
  - [x] 7.5 Implement saveTile function
    - Save PNG to cache with directory creation
    - Create intermediate directories as needed
    - _Requirements: 8.1, 8.2, 19.4_
  
  - [x] 7.6 Write property test for cache consistency
    - **Property 18: Cache lookup**
    - **Validates: Requirements 8.3, 8.4**
  
  - [x] 7.7 Write unit tests for cache operations
    - Test directory creation, file I/O, cache hits/misses
    - Test cache cleanup
    - _Requirements: 8.1, 8.2, 19.1, 19.5_

- [x] 8. Implement MetadataManager for metadata persistence
  - [x] 8.1 Implement metadata.json schema
    - Store: file path, size, page length, block size, timestamp
    - _Requirements: 22.1, 22.5_
  
  - [x] 8.2 Implement loadMetadata function
    - Load metadata.json from .cache/{dump_filename}/
    - Validate metadata is still valid
    - _Requirements: 22.2, 22.3_
  
  - [x] 8.3 Implement saveMetadata function
    - Save metadata.json with current parameters
    - _Requirements: 22.1, 22.6_
  
  - [x] 8.4 Write unit tests for metadata persistence
    - Test save/load cycle
    - Test validation of stale metadata
    - _Requirements: 22.1, 22.3_

- [x] 9. Implement TaskQueue with priority-based task management
  - [x] 9.1 Define Priority enum (High, Normal, Low)
    - _Requirements: 9.1_
  
  - [x] 9.2 Implement TaskQueue with three priority levels
    - Use three separate queues or priority-ordered structure
    - _Requirements: 9.1, 9.6_
  
  - [x] 9.3 Implement enqueue function (thread-safe)
    - Insert task into appropriate priority queue
    - _Requirements: 9.1_
  
  - [x] 9.4 Implement dequeue function (thread-safe)
    - Return highest priority task (high → normal → low)
    - _Requirements: 9.6_
  
  - [x] 9.5 Implement updatePriority function
    - Update priority of existing task
    - _Requirements: 9.2, 9.3, 9.4, 9.5_
  
  - [x] 9.6 Implement remove function
    - Remove task from queue
    - _Requirements: 9.2, 9.3_
  
  - [x] 9.7 Write property test for priority ordering
    - **Property 21: Task queue priority levels**
    - **Validates: Requirements 9.1, 9.6**
  
  - [x] 9.8 Write property test for thread-safe queue access
    - **Property 24: Thread-safe queue access**
    - **Validates: Requirements 9.7**
  
  - [x] 9.9 Write unit tests for task queue operations
    - Test enqueue/dequeue, priority ordering, concurrent access
    - _Requirements: 9.1, 9.6, 9.7_

- [-] 10. Implement TileGenerator for high-resolution tile generation
  - [x] 10.1 Implement calculateFragments function
    - Given tile coordinate, calculate byte ranges needed from dump
    - Account for page/block layout and grid positioning
    - _Requirements: 6.1, 6.2_
  
  - [x] 10.2 Write property test for fragment calculation
    - **Property 16: Fragment calculation**
    - **Validates: Requirements 6.1, 6.2**
  
  - [x] 10.3 Implement generateTile function
    - Load fragments from file
    - Render using BitRenderer, ByteArranger, BlockArranger
    - Encode as PNG
    - _Requirements: 6.3_
  
  - [x] 10.4 Write unit tests for tile generation
    - Test fragment loading and rendering
    - Test PNG output validity
    - _Requirements: 6.1, 6.3_

- [x] 11. Implement PyramidTileGenerator for lower-resolution tile composition
  - [x] 11.1 Implement pyramid level calculation
    - Calculate tile dimensions at each level
    - Determine when pyramid terminates (entire dump in one tile)
    - _Requirements: 5.1, 5.2, 5.3, 5.4_
  
  - [x] 11.2 Write property test for pyramid level organization
    - **Property 12: Pyramid level organization**
    - **Validates: Requirements 5.1, 5.2, 5.3**
  
  - [x] 11.3 Implement compositeTiles function
    - Combine 4 tiles into 2x2 grid
    - _Requirements: 7.3_
  
  - [x] 11.4 Implement downscale function
    - Downscale composited tile to half resolution using 2:1 pixel averaging
    - _Requirements: 7.4_
  
  - [x] 11.5 Implement generatePyramidTile function
    - Identify 4 child tiles at level-1
    - Load or request children (high priority if missing)
    - Composite and downscale
    - Cache result
    - _Requirements: 7.1, 7.2, 7.3, 7.4, 7.5_
  
  - [x] 11.6 Write property test for pyramid composition strategy
    - **Property 14: Pyramid composition strategy**
    - **Validates: Requirements 5.5**
  
  - [x] 11.7 Write unit tests for pyramid tile generation
    - Test composition, downscaling, caching
    - _Requirements: 7.1, 7.3, 7.4, 7.5_

- [x] 12. Implement WorkerPool for parallel tile generation
  - [x] 12.1 Implement WorkerPool::new
    - Create one worker per available CPU core (minimum 1)
    - _Requirements: 10.1, 10.2_
  
  - [x] 12.2 Implement worker thread function
    - Continuously dequeue tasks from queue
    - Generate tile (high-res or pyramid based on level)
    - Cache result
    - Mark task complete
    - Enter wait state if queue empty
    - _Requirements: 10.3, 10.4, 10.5, 10.6_
  
  - [x] 12.3 Implement start function
    - Spawn all worker threads
    - _Requirements: 10.1, 10.2_
  
  - [x] 12.4 Implement shutdown function
    - Gracefully stop all workers
    - _Requirements: 10.1_
  
  - [x] 12.5 Write unit tests for worker pool
    - Test worker creation, task processing, shutdown
    - _Requirements: 10.1, 10.2, 10.3, 10.5_

- [x] 13. Implement ViewportManager for viewport-based tile prioritization
  - [x] 13.1 Implement updateViewport function
    - Update viewport state (level, center, dimensions)
    - Recalculate visible and adjacent tiles
    - _Requirements: 11.1, 11.2, 11.3_
  
  - [x] 13.2 Implement getVisibleTiles function
    - Return tiles currently in viewport
    - _Requirements: 11.1, 11.2_
  
  - [x] 13.3 Implement getAdjacentTiles function
    - Return tiles adjacent to viewport (predictive loading)
    - _Requirements: 11.3_
  
  - [x] 13.4 Implement updateTaskPriorities function
    - Assign high priority to visible tiles
    - Assign normal priority to adjacent tiles
    - Assign low priority to others
    - _Requirements: 9.2, 9.3, 9.4, 9.5, 11.1, 11.2, 11.3_
  
  - [x] 13.5 Write property test for viewport tile identification
    - **Property 29: Viewport tile identification**
    - **Validates: Requirements 11.1, 11.2**
  
  - [x] 13.6 Write unit tests for viewport management
    - Test visible/adjacent tile calculation
    - Test priority updates
    - _Requirements: 11.1, 11.2, 11.3, 11.4, 11.5_

- [x] 14. Implement ZoomController for zoom operations
  - [x] 14.1 Implement zoomIn function
    - Increase zoom level (more pixels per bit)
    - Maintain viewport center
    - _Requirements: 12.1, 12.3_
  
  - [x] 14.2 Implement zoomOut function
    - Decrease zoom level (fewer pixels per bit)
    - Maintain viewport center
    - _Requirements: 12.2, 12.3_
  
  - [x] 14.3 Implement zoom level constraints
    - Default: 1 bit = 1 pixel
    - Maximum: 1 bit = 16x16 pixels (256 pixels per bit)
    - Minimum: entire dump fits in quarter-screen
    - _Requirements: 12.6, 12.7, 12.8_
  
  - [x] 14.4 Implement continuous zoom support
    - Support fractional zoom levels (not just discrete steps)
    - _Requirements: 12.4_
  
  - [x] 14.5 Implement tile request on zoom
    - Request tiles for new viewport after zoom
    - _Requirements: 12.5_
  
  - [x] 14.6 Write unit tests for zoom controller
    - Test zoom in/out, center preservation, constraints
    - _Requirements: 12.1, 12.2, 12.3, 12.6, 12.7, 12.8_

- [x] 15. Implement PanController for pan operations
  - [x] 15.1 Implement pan function
    - Update viewport coordinates
    - Request tiles for new viewport
    - _Requirements: 13.1, 13.2_
  
  - [x] 15.2 Implement boundary enforcement
    - Prevent panning beyond dump bounds
    - _Requirements: 13.4_
  
  - [x] 15.3 Implement smooth panning
    - Support smooth pan without visible gaps
    - _Requirements: 13.3_
  
  - [x] 15.4 Write unit tests for pan controller
    - Test pan operations, boundary enforcement
    - _Requirements: 13.1, 13.2, 13.4_

- [x] 16. Implement AddressDisplay for mouse position tracking
  - [x] 16.1 Implement updateMousePosition function
    - Convert screen coordinates to dump coordinates
    - Calculate block, page, byte, bit address
    - Account for zoom level and viewport position
    - _Requirements: 21.1, 21.5_
  
  - [x] 16.2 Implement getAddress function
    - Return formatted address "Block: X, Page: Y, Byte: Z, Bit: W"
    - _Requirements: 21.3_
  
  - [x] 16.3 Implement isMouseInBounds function
    - Return true if mouse is over visualization
    - _Requirements: 21.4_
  
  - [x] 16.4 Write property test for address calculation
    - **Property 54: Mouse position address calculation**
    - **Validates: Requirements 21.1, 21.5**
  
  - [x] 16.5 Write unit tests for address display
    - Test address calculation at various positions
    - Test out-of-bounds handling
    - _Requirements: 21.1, 21.3, 21.4_

- [-] 17. Implement error handling and retry logic
  - [x] 17.1 Implement exponential backoff retry
    - Retry failed tiles with delay = baseDelay * (2 ^ retryCount)
    - Maximum 3 retries
    - _Requirements: 16.1, 16.2_
  
  - [x] 17.2 Implement placeholder tile generation
    - Generate placeholder for tiles that fail after max retries
    - _Requirements: 16.3_
  
  - [x] 17.3 Implement non-blocking error handling
    - Ensure tile failures don't block other tile generation
    - _Requirements: 16.4_
  
  - [x] 17.4 Implement error logging
    - Log errors with context (tile coordinates, reason)
    - _Requirements: 16.1_
  
  - [x] 17.5 Write unit tests for error handling
    - Test retry logic, placeholder display, non-blocking behavior
    - _Requirements: 16.1, 16.2, 16.3, 16.4_

- [x] 18. Implement file open dialog and parameter input
  - [x] 18.1 Create file open dialog UI
    - Allow user to select dump file
    - _Requirements: 1.1_
  
  - [x] 18.2 Implement parameter input form
    - Prompt for page length (500-20000 bytes)
    - Prompt for block size (64, 128, 256, 512, 1024 pages)
    - _Requirements: 1.2, 1.3, 15.1, 15.2_
  
  - [x] 18.3 Implement parameter validation
    - Validate page length range
    - Validate block size values
    - Display error messages for invalid input
    - _Requirements: 15.3, 15.4_
  
  - [x] 18.4 Implement metadata caching in dialog
    - Check for cached metadata before prompting
    - Load cached metadata if valid
    - Allow user to override cached values
    - _Requirements: 22.1, 22.2, 22.3, 22.6_

- [x] 19. Implement main application window and viewport rendering
  - [x] 19.1 Create main window UI
    - Display viewport with rendered tiles
    - Show status bar with address display
    - _Requirements: 21.6_
  
  - [x] 19.2 Implement tile rendering in viewport
    - Load tiles from cache or request from queue
    - Composite tiles into viewport
    - Handle missing/loading tiles gracefully
    - _Requirements: 8.3, 8.4_
  
  - [x] 19.3 Implement mouse event handling
    - Track mouse position for address display
    - Handle zoom/pan input (scroll wheel, drag)
    - _Requirements: 21.1, 21.2, 12.1, 12.2, 13.1_
  
  - [x] 19.4 Implement initial viewport setup
    - Position at upper left corner (first page, first byte)
    - Start at default zoom level (1 bit = 1 pixel)
    - Request initial tiles
    - _Requirements: 14.1, 14.2, 14.3, 14.4_

- [x] 20. Implement multi-file support
  - [x] 20.1 Implement window/tab management
    - Support opening multiple dump files in separate windows or tabs
    - _Requirements: 20.1_
  
  - [x] 20.2 Implement per-file cache isolation
    - Each dump file has own cache directory (.cache/{dump_filename}/)
    - _Requirements: 20.2_
  
  - [x] 20.3 Implement per-file worker pool
    - Each dump file has own worker pool and task queues
    - _Requirements: 20.3_
  
  - [x] 20.4 Implement state preservation
    - Preserve viewport position and zoom level for each dump
    - _Requirements: 20.4_
  
  - [x] 20.5 Write unit tests for multi-file support
    - Test window management, cache isolation, state preservation
    - _Requirements: 20.1, 20.2, 20.3, 20.4_

- [x] 21. Checkpoint - Ensure all core components are integrated
  - Verify all components compile without errors
  - Verify basic tile generation works end-to-end
  - Ensure all tests pass
  - Ask the user if questions arise.

- [ ] 22. Implement immediate startup validation
  - [ ] 22.1 Measure file open to first display time
    - Verify < 500ms startup time
    - _Requirements: 1.4, 17.1, 17.2, 17.3_
  
  - [ ] 22.2 Verify no file scanning occurs
    - Confirm only first block is read for metadata
    - _Requirements: 17.1_
  
  - [ ] 22.3 Verify no preprocessing occurs
    - Confirm tiles generated on-demand only
    - _Requirements: 17.2_
  
  - [ ] 22.4 Write performance test for startup time
    - Measure file open to first display
    - Verify < 500ms
    - _Requirements: 1.4, 17.1, 17.2, 17.3_

- [ ] 23. Implement property-based test suite
  - [ ] 23.1 Set up property-based testing framework (proptest)
    - Configure minimum 100 iterations per property test
    - _Requirements: All_
  
  - [ ] 23.2 Implement all property tests from design document
    - Properties 1-58 as defined in design
    - Tag each with feature and property number
    - _Requirements: All_
  
  - [ ] 23.3 Run full property test suite
    - Execute all property tests
    - Verify all pass
    - _Requirements: All_

- [x] 24. Implement integration tests
  - [x] 24.1 Test file load → metadata detection → tile generation
    - End-to-end flow from file open to first tile display
    - _Requirements: 1.1, 1.2, 1.3, 1.4, 1.6_
  
  - [x] 24.2 Test viewport change → priority update → tile generation
    - Verify viewport-driven loading works
    - _Requirements: 11.1, 11.2, 11.3, 11.4, 11.5_
  
  - [x] 24.3 Test zoom/pan → viewport update → tile requests
    - Verify UI interaction flow
    - _Requirements: 12.1, 12.2, 12.3, 12.5, 13.1, 13.2_
  
  - [x] 24.4 Test cache hit → tile display
    - Verify cached tiles are used
    - _Requirements: 8.3, 8.4_
  
  - [x] 24.5 Test cache miss → generation → caching
    - Verify new tiles are generated and cached
    - _Requirements: 6.3, 8.1, 8.2_
  
  - [x] 24.6 Test multiple workers → concurrent generation
    - Verify parallel processing works correctly
    - _Requirements: 10.1, 10.2, 10.3, 10.4, 10.5_
  
  - [x] 24.7 Run full integration test suite
    - Execute all integration tests
    - Verify all pass
    - _Requirements: All_

- [ ] 25. Final checkpoint - Ensure all tests pass
  - Ensure all unit tests pass
  - Ensure all property tests pass
  - Ensure all integration tests pass
  - Verify no compiler warnings
  - Ask the user if questions arise.

## Notes

- Tasks marked with `*` are optional and can be skipped for faster MVP
- Each task references specific requirements for traceability
- Property tests validate universal correctness properties
- Unit tests validate specific examples and edge cases
- Integration tests validate component interactions
- Checkpoints ensure incremental validation
- All code must compile without errors or warnings
- Implementation language: Rust
