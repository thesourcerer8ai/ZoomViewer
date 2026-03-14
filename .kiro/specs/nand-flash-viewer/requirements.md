# Requirements Document: NAND Flash Viewer

## Introduction

The NAND Flash Viewer is a high-performance image viewer designed to handle extremely large NAND flash dump files (50-500 GB) with immediate startup and responsive zoom/pan capabilities. The system uses an image pyramid algorithm similar to Google Maps to enable efficient navigation of massive binary data files without preprocessing. Users can open a dump file and immediately visualize the data at various zoom levels, with intelligent tile caching and worker-based parallel processing.

## Glossary

- **NAND_Dump**: A binary file containing sequential NAND flash pages, ranging from 50-500 GB in size
- **Page**: A fixed-length unit of data within a NAND dump (usually 500-20000 bytes), all pages in a dump have identical length
- **Block**: A group of consecutive pages (usually 64, 128, 256, 512, or 1024 pages per block)
- **Tile**: A PNG image representing a rectangular region of the visualization at a specific zoom level
- **Pyramid**: A multi-level image hierarchy where each level is half the resolution of the level below
- **Viewport**: The currently visible rectangular region of the visualization on screen
- **Fragment**: A contiguous byte range from the NAND dump file required to generate a tile
- **Worker**: A background process that generates tiles from dump data, one per CPU core
- **Task_Queue**: A priority-based queue holding tile generation requests (high, normal, low priority)
- **Cache**: The ".cache" directory storing generated PNG tiles for reuse
- **Resolution_Level**: A zoom level in the pyramid, where level 0 is the highest resolution (individual bits)
- **Bit_Representation**: Visual encoding where 1 is black and 0 is white
- **Byte_Order**: Horizontal arrangement where first byte is on left, last byte is on right
- **Block_Order**: Vertical arrangement where first page is on top, last page is on bottom

## Requirements

### Requirement 1: Load NAND Dump Files with User-Provided Parameters

**User Story:** As a user, I want to open a NAND flash dump file with page length and block size parameters, so that I can visualize its contents.

#### Acceptance Criteria

1. WHEN a NAND dump file is selected, THE Viewer SHALL accept files of any size
2. WHEN a dump file is opened, THE Viewer SHALL require the user to provide the page length (500-20000 bytes)
3. WHEN a dump file is opened, THE Viewer SHALL require the user to provide the block size (64, 128, 256, 512, or 1024 pages per block)
4. WHEN a dump file is opened, THE Viewer SHALL immediately display the visualization without preprocessing the entire file
5. IF the dump file cannot be read or is corrupted, THEN THE Viewer SHALL display an error message with the specific I/O error details
6. WHEN a dump file is loaded, THE Viewer SHALL store file metadata (size, page length, block size) in memory for tile generation

### Requirement 2: Visualize Bits as Black and White Pixels

**User Story:** As a user, I want to see individual bits rendered as pixels, so that I can inspect the raw binary data.

#### Acceptance Criteria

1. THE Bit_Renderer SHALL display each bit as a single pixel
2. THE Bit_Renderer SHALL render bit value 1 as black pixels
3. THE Bit_Renderer SHALL render bit value 0 as white pixels
4. THE Bit_Renderer SHALL read bits in LSB-first order (rightmost bit is LSB, leftmost bit is MSB)
5. WHEN rendering a byte, THE Bit_Renderer SHALL arrange bits horizontally from MSB (left) to LSB (right)

### Requirement 3: Arrange Bytes Horizontally

**User Story:** As a user, I want to see bytes arranged left-to-right, so that I can follow the natural reading order of the dump.

#### Acceptance Criteria

1. THE Byte_Arranger SHALL display bytes horizontally in sequence
2. THE Byte_Arranger SHALL place the first byte of a page on the left
3. THE Byte_Arranger SHALL place the last byte of a page on the right
4. WHEN rendering a page, THE Byte_Arranger SHALL maintain consistent byte spacing

### Requirement 4: Arrange Pages and Blocks Vertically

**User Story:** As a user, I want to see pages stacked vertically with blocks clearly separated, so that I can navigate the dump structure.

#### Acceptance Criteria

1. THE Block_Arranger SHALL display pages vertically in sequence within each block
2. THE Block_Arranger SHALL place the first page of a block at the top
3. THE Block_Arranger SHALL place the last page of a block at the bottom
4. THE Block_Arranger SHALL add visible spacing between consecutive pages
5. THE Block_Arranger SHALL add larger visible spacing between consecutive blocks
6. WHEN rendering multiple blocks, THE Block_Arranger SHALL arrange blocks in a grid layout (top-to-bottom, left-to-right)
7. THE Block_Arranger SHALL maintain a grid aspect ratio of approximately 4:3 (width:height)

### Requirement 5: Implement Image Pyramid for Zoom Levels

**User Story:** As a developer, I want the system to use an image pyramid algorithm, so that zoom and pan operations are efficient.

#### Acceptance Criteria

1. THE Pyramid SHALL organize tiles into multiple resolution levels
2. THE Pyramid SHALL use level 0 as the highest resolution (individual bits visible)
3. WHEN moving to a lower resolution level, THE Pyramid SHALL reduce dimensions by half in both width and height
4. THE Pyramid SHALL continue creating levels until the entire dump fits in a single tile
5. WHEN a tile at a lower resolution level is needed, THE Pyramid_Generator SHALL generate it from tiles in the level below (not from the dump directly)
6. FOR ALL tiles in the pyramid, THE Pyramid SHALL maintain consistent tile dimensions across all levels

### Requirement 6: Generate Tiles from Dump Fragments

**User Story:** As a system component, I want to generate high-resolution tiles from dump data, so that the visualization can display raw NAND content.

#### Acceptance Criteria

1. WHEN a high-resolution tile is requested, THE Tile_Generator SHALL calculate which byte ranges (fragments) from the dump are needed
2. THE Tile_Generator SHALL load only the required fragments from the dump file
3. THE Tile_Generator SHALL render the fragments into a PNG tile using the bit/byte/block arrangement rules
4. WHEN tile generation completes, THE Tile_Generator SHALL cache the PNG in the ".cache" directory
5. IF a fragment cannot be read from the dump, THEN THE Tile_Generator SHALL mark the tile as failed and report the error

### Requirement 7: Generate Pyramid Tiles from Lower Levels

**User Story:** As a system component, I want to generate lower-resolution tiles from higher-resolution tiles, so that zoom-out operations are efficient.

#### Acceptance Criteria

1. WHEN a pyramid tile is requested, THE Pyramid_Tile_Generator SHALL load tiles from the resolution level below
2. IF a required tile from the lower level is not cached, THEN THE Pyramid_Tile_Generator SHALL send a high-priority request to the Task_Queue for that tile
3. WHEN all required lower-level tiles are available, THE Pyramid_Tile_Generator SHALL composite them into a single tile
4. THE Pyramid_Tile_Generator SHALL downscale the composited tile to half resolution
5. WHEN pyramid tile generation completes, THE Pyramid_Tile_Generator SHALL cache the PNG in the ".cache" directory

### Requirement 8: Cache Generated Tiles

**User Story:** As a system component, I want to cache tiles, so that repeated access to the same region is fast.

#### Acceptance Criteria

1. THE Cache SHALL store all generated PNG tiles in the ".cache" directory
2. THE Cache SHALL use a hierarchical directory structure: ".cache/{dump_filename}/{level}/{block_y}/{block_x}.png"
3. WHEN a tile is requested, THE Cache SHALL check if it exists before generating
4. IF a tile exists in the cache, THE Cache SHALL load it instead of regenerating
5. THE Cache SHALL support cache invalidation when the dump file is modified

### Requirement 9: Implement Task Queue with Priority Levels

**User Story:** As a system component, I want to prioritize tile generation, so that the viewport is always responsive.

#### Acceptance Criteria

1. THE Task_Queue SHALL maintain three priority levels: high, normal, and low
2. WHEN a tile enters the viewport, THE Task_Queue SHALL assign it high priority
3. WHEN a tile exits the viewport, THE Task_Queue SHALL allow its high-priority status to be withdrawn
4. WHEN a tile is outside the viewport but adjacent, THE Task_Queue SHALL assign it normal priority
5. WHEN a tile is far from the viewport, THE Task_Queue SHALL assign it low priority
6. WHEN workers check the queue, THE Worker SHALL process high-priority tasks first, then normal, then low
7. THE Task_Queue SHALL support concurrent access from multiple workers without data corruption

### Requirement 10: Distribute Work Across CPU Cores

**User Story:** As a system component, I want to use all available CPU cores, so that tile generation is parallelized.

#### Acceptance Criteria

1. THE Worker_Pool SHALL create one worker per available CPU core
2. WHEN the system starts, THE Worker_Pool SHALL spawn all workers
3. EACH Worker SHALL continuously check the Task_Queue for pending tiles
4. EACH Worker SHALL process tiles in priority order (high → normal → low)
5. WHEN a worker completes a tile, THE Worker SHALL cache it and mark the task as complete
6. WHEN the Task_Queue is empty, EACH Worker SHALL enter a low-power wait state

### Requirement 11: Implement Viewport-Based Tile Loading

**User Story:** As a system component, I want to load tiles based on the current viewport, so that memory usage is bounded.

#### Acceptance Criteria

1. WHEN the viewport changes, THE Viewport_Manager SHALL identify which tiles are now visible
2. THE Viewport_Manager SHALL assign high priority to all tiles in the viewport
3. THE Viewport_Manager SHALL assign normal priority to tiles adjacent to the viewport (predictive loading)
4. WHEN the user pans away from a tile, THE Viewport_Manager SHALL allow its high-priority status to be withdrawn
5. THE Viewport_Manager SHALL update priorities in real-time as the user pans and zooms

### Requirement 12: Support Zoom Operations

**User Story:** As a user, I want to zoom in and out, so that I can inspect details or see the big picture.

#### Acceptance Criteria

1. WHEN the user zooms in, THE Zoom_Controller SHALL increase the zoom level (more pixels per bit)
2. WHEN the user zooms out, THE Zoom_Controller SHALL decrease the zoom level (fewer pixels per bit)
3. WHEN zooming, THE Zoom_Controller SHALL maintain the center point of the viewport
4. THE Zoom_Controller SHALL support continuous zoom levels (not just discrete steps)
5. WHEN a new zoom level is reached, THE Zoom_Controller SHALL request tiles for the new viewport
6. THE default zoom level SHALL be 1 bit equals 1 pixel
7. THE maximum zoom level SHALL be 1 bit equals 16x16 pixels (256 pixels per bit)
8. THE minimum zoom level SHALL be such that the entire dump visualization fits in a quarter of the screen

### Requirement 13: Support Pan Operations

**User Story:** As a user, I want to pan across the visualization, so that I can explore different regions of the dump.

#### Acceptance Criteria

1. WHEN the user pans, THE Pan_Controller SHALL update the viewport coordinates
2. THE Pan_Controller SHALL request tiles for the new viewport region
3. THE Pan_Controller SHALL support smooth panning without visible gaps
4. WHEN panning reaches the edge of the dump, THE Pan_Controller SHALL prevent further panning beyond bounds

### Requirement 14: Start at Upper Left Corner with Highest Resolution

**User Story:** As a user, I want the viewer to start at a sensible default position, so that I can immediately see data.

#### Acceptance Criteria

1. WHEN a dump file is opened, THE Viewer SHALL position the viewport at the upper left corner (first page, first byte)
2. WHEN a dump file is opened, THE Viewer SHALL start at the highest available resolution level
3. WHEN a dump file is opened, THE Viewer SHALL immediately request tiles for the initial viewport
4. WHEN initial tiles are loaded, THE Viewer SHALL display them as soon as they are available

### Requirement 15: Accept User-Provided Page and Block Sizes

**User Story:** As a system component, I want to accept page and block sizes from the user, so that the viewer can handle any NAND dump configuration.

#### Acceptance Criteria

1. WHEN a dump file is opened, THE Viewer SHALL prompt the user for page length (500-20000 bytes)
2. WHEN a dump file is opened, THE Viewer SHALL prompt the user for block size (64, 128, 256, 512, or 1024 pages per block)
3. WHEN the user provides invalid values, THE Viewer SHALL display an error message and request valid values
4. WHEN valid values are provided, THE Viewer SHALL store them in file metadata for tile generation
5. THE Viewer SHALL validate that page length is within 500-20000 bytes
6. THE Viewer SHALL validate that block size is one of: 64, 128, 256, 512, or 1024 pages per block

### Requirement 22: Cache Metadata for Repeated File Access

**User Story:** As a user, I want the viewer to remember the page length and block size, so that I don't have to re-enter them when opening the same dump file again.

#### Acceptance Criteria

1. WHEN a dump file is opened with valid parameters, THE Viewer SHALL cache the metadata in ".cache/{dump_filename}/metadata.json"
2. WHEN a dump file is opened, THE Viewer SHALL check for cached metadata before prompting the user
3. IF cached metadata exists and is valid, THE Viewer SHALL load it automatically without user input
4. IF cached metadata is invalid or missing, THE Viewer SHALL prompt the user for parameters
5. THE cached metadata SHALL include: file path, size, page length, block size, and timestamp
6. THE Viewer SHALL support updating cached metadata if the user provides different parameters

### Requirement 16: Handle Tile Generation Failures Gracefully

**User Story:** As a system component, I want to handle errors during tile generation, so that the viewer remains responsive.

#### Acceptance Criteria

1. IF a tile generation fails, THEN THE Error_Handler SHALL log the error with context (tile coordinates, reason)
2. IF a tile generation fails, THEN THE Error_Handler SHALL retry the tile with exponential backoff
3. IF a tile fails after maximum retries, THEN THE Error_Handler SHALL display a placeholder tile indicating the error
4. WHEN a tile fails, THE Error_Handler SHALL NOT block other tile generation tasks

### Requirement 17: Provide Immediate Startup Without Preprocessing

**User Story:** As a user, I want the viewer to start immediately, so that I don't wait for file analysis.

#### Acceptance Criteria

1. WHEN a dump file is opened, THE Viewer SHALL NOT scan the entire file
2. WHEN a dump file is opened, THE Viewer SHALL NOT precompute any tiles
3. WHEN a dump file is opened, THE Viewer SHALL display the initial viewport within 500ms
4. THE Viewer SHALL load file metadata (size, page length, block size) on-demand as tiles are generated

### Requirement 18: Parse and Pretty-Print Tile Coordinates

**User Story:** As a developer, I want to convert between tile coordinates and dump offsets, so that tile generation is accurate.

#### Acceptance Criteria

1. THE Coordinate_Parser SHALL convert tile coordinates (level, x, y) to byte offsets in the dump
2. THE Coordinate_Parser SHALL convert byte offsets to tile coordinates
3. WHEN parsing coordinates, THE Coordinate_Parser SHALL account for page length, block size, and grid layout
4. FOR ALL valid tile coordinates, parsing then converting back SHALL produce equivalent coordinates (round-trip property)
5. THE Pretty_Printer SHALL format tile coordinates as human-readable strings for logging and debugging

### Requirement 19: Manage Cache Directory Structure

**User Story:** As a system component, I want to organize cached tiles, so that they are easy to locate and manage.

#### Acceptance Criteria

1. THE Cache_Manager SHALL create the ".cache" directory if it does not exist
2. THE Cache_Manager SHALL organize tiles in subdirectories by resolution level: ".cache/{level}/"
3. THE Cache_Manager SHALL organize tiles by block coordinates: ".cache/{level}/{block_y}/{block_x}.png"
4. WHEN a tile is cached, THE Cache_Manager SHALL create intermediate directories as needed
5. THE Cache_Manager SHALL support cache cleanup to remove old or unused tiles

### Requirement 20: Support Multiple Concurrent Dump Files

**User Story:** As a user, I want to open multiple dump files, so that I can compare different NAND dumps.

#### Acceptance Criteria

1. THE Viewer SHALL support opening multiple dump files in separate windows or tabs
2. EACH dump file SHALL have its own cache directory: ".cache/{dump_id}/"
3. EACH dump file SHALL have its own worker pool and task queues
4. WHEN switching between dumps, THE Viewer SHALL preserve the viewport position and zoom level for each dump

### Requirement 21: Display Mouse Position Address

**User Story:** As a user, I want to see the block/page/byte/bit address where my mouse is pointing, so that I can identify specific locations in the dump.

#### Acceptance Criteria

1. WHEN the mouse moves over the visualization, THE Address_Display SHALL calculate the block, page, byte, and bit address at the mouse position
2. THE Address_Display SHALL continuously update the displayed address as the mouse moves
3. THE Address_Display SHALL format the address as: "Block: X, Page: Y, Byte: Z, Bit: W"
4. WHEN the mouse is outside the visualization bounds, THE Address_Display SHALL display "N/A" or hide the address
5. THE Address_Display SHALL account for the current zoom level and viewport position when calculating addresses
6. THE Address_Display SHALL be visible in the UI at all times (e.g., in a status bar or tooltip)

