# Zoom Fix Plan

## Problem Analysis

The zoom functionality has several issues:

1. **Coordinate System Confusion**: The zoom controller receives mouse coordinates in screen space but needs to work in world space
2. **Center Calculation**: When zooming, the viewport center needs to be adjusted to keep the mouse position fixed in world space
3. **Missing Pyramid Tiles**: Pyramid tiles aren't being generated when zooming out

## Solution

### Fix 1: Proper Coordinate Transformation

When zooming at a specific point:
1. Get current viewport center in world coordinates (from ViewportManager)
2. Convert mouse position from screen space to world space
3. Calculate offset from viewport center to mouse position
4. Apply zoom factor
5. Calculate new viewport center to maintain mouse position

### Fix 2: Ensure Task Priorities Are Updated

After zoom, ensure `update_task_priorities()` is called to enqueue tiles at the new level.

### Fix 3: Verify Worker Logic

Ensure workers are correctly generating pyramid tiles when requested.

## Implementation

The simplest fix is to:
1. Get the current viewport from ViewportManager
2. Calculate mouse position in world coordinates
3. Update zoom factor
4. Calculate new viewport center
5. Update ViewportManager with new center and level
