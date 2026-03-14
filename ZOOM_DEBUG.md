# Zoom Debugging Notes

## Current Status
- Only level 0 tiles exist in cache
- When zooming out, no pyramid tiles (level 1, 2, etc.) are being generated
- Viewport shows gray placeholders when zoomed out

## Expected Behavior
1. User scrolls mouse wheel down (zoom out)
2. ZoomController calculates new zoom factor (< 1.0)
3. ZoomController calculates new level (e.g., level 1)
4. ZoomController updates ViewportManager with new level
5. ViewportManager calculates visible tiles at new level
6. ViewportManager enqueues tile generation tasks for missing tiles
7. Workers generate pyramid tiles from level 0 tiles
8. Tiles appear as they're generated

## Potential Issues
1. **Viewport not updating after zoom** - Fixed in app_window.rs event handler
2. **Task priorities not being updated** - Need to verify update_task_priorities() is called
3. **Workers not generating pyramid tiles** - Need to verify worker logic
4. **Pyramid tile generation failing** - Need to check for errors

## Next Steps
1. Add logging to see what level is being requested
2. Check if tasks are being enqueued at the correct level
3. Verify workers are processing pyramid tile tasks
4. Check for any errors in pyramid tile generation
