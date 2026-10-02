# Viewport Layout Review — Fixes Summary

**Commit:** (see git log)
**Base commit:** ca62ac4
**File changed:** `src/app_window.rs`

---

## Issue 1 — Mouse Y not adjusted for tab strip in zoom-on-cursor (BLOCKING)

### Problem
`Event::MouseWheel` handler computed `mouse_y = fltk::app::event_y() as f64`, which
is window-relative and includes the 30 px tab strip at the top.
`ZoomController::screen_height` was already set to the usable viewport height
(excluding the tab strip), so the pivot calculation inside
`update_viewport_after_zoom` (`mouse_screen_y − screen_center_y`) was biased 30 px
above the true cursor position.

### Fix
At the MouseWheel call site (around line 1462) subtracted the tab strip offset before
forwarding the coordinate to `zoom_ctrl.zoom_in`/`zoom_out`:

```rust
let tab_strip_offset: f64 = 30.0;
let mouse_y = fltk::app::event_y() as f64 - tab_strip_offset;
```

No other call site forwards `event_y()` to `ZoomController` or `PanController`
with a zoom-pivot interpretation.  The pan handler (`Event::Drag`) uses `event_y()`
only to compute a pixel delta (`current_y - last_mouse_y`), so no offset is needed
there.  The Ctrl+0/+/- keyboard handler already computes a `center_y` that
accounts for the tab strip.

---

## Issue 2 — Hardcoded initial height 648 instead of correct 678 (BLOCKING)

### Problem
Five construction-time call sites used the hardcoded value `648` as the initial
viewport height.  The correct usable height for the 768 px default window is
`768 − 30 − 60 = 678`.  The 30 px shortfall left controllers miscalibrated on
cold start; they snapped to the right size only on the first resize event.

### Fix
The layout constants `tab_strip_h = 30`, `status_bar_h = 60`, and their derived
`viewport_h = saved_win.height − tab_strip_h − status_bar_h` were already
computed near the top of `AppWindow::new()`.  All five hardcoded `648` values were
replaced with expressions derived from these variables:

| Call site (approx. line) | Before | After |
|---|---|---|
| `ZoomController::new(…, 1024, 648)` | `1024, 648` | `saved_win.width as u32, viewport_h as u32` |
| `PanController::new(…, 1024, 648)` | `1024, 648` | `saved_win.width as u32, viewport_h as u32` |
| `vm.update_viewport(0, …, 1024, 648)` (construction) | `1024, 648` | `saved_win.width as u32, viewport_h as u32` |
| `RgbaImage::new(1024, 648)` | `1024, 648` | `saved_win.width as u32, viewport_h as u32` |
| `vm.update_viewport(0, …, 1024, 648)` (timer — new output provider) | `1024, 648` | `initial_viewport_w as u32, initial_viewport_h as u32` |

For the timer closure (which cannot see `saved_win` or `viewport_h` directly),
two capture variables were introduced immediately before the timer setup:

```rust
let initial_viewport_w = saved_win.width;   // i32, Copy
let initial_viewport_h = viewport_h;        // i32, Copy
```

---

## Test Results

```
test result: ok. 435 passed; 0 failed; 8 ignored
```

All 435 tests pass.  No existing tests were modified.
