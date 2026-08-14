# Progress

## 2026-08-13 (branch `initial-model-viewer`) — egui menu bar and loading spinner

Evaluated GPUI/gpui-component first: rejected because GPUI owns its own window and
event loop, so its widgets cannot be drawn into a Bevy window. `paint_surface` is
macOS-only (`gpui/src/window.rs`, `#[cfg(target_os = "macos")]`) and takes a
`CVPixelBuffer`, which would force a GPU→CPU→GPU roundtrip per frame. Chose
`bevy_egui` instead, which is built for overlaying UI on a Bevy scene.

- Added `bevy_egui` 0.40.1 (not 0.41.1: `bevy_panorbit_camera` 0.35's `bevy_egui`
  feature requires `^0.40`; mixing would pull two egui versions and silently break
  `EguiWantsFocus`). `cargo tree -i egui` confirms a single egui 0.34.3.
- Enabled `bevy_panorbit_camera`'s `bevy_egui` feature and set
  `EguiFocusIncludesHover(true)` so hovering the menu bar does not orbit the camera.
- New `draw_ui` system on `EguiPrimaryContextPass`: File menu (Open… / Quit) plus a
  centered status overlay with `egui::Spinner` while loading.
- New `OpenFileRequested` message decouples the trigger from the dialog. The `O`
  shortcut and the menu item both write it; `open_file_dialog` reads it. Repeats
  within a frame collapse to one dialog.
- Removed the hand-rolled Bevy UI loading overlay: `LoadingPanel` / `LoadingText`
  components and their spawn block. `sync_window_title_and_loading_ui` shrank to
  `sync_window_title`; the label logic moved to `status_label`.
- `Panel::show` is deprecated in egui 0.34, so the menu bar uses `show_inside` on a
  background-layer `Ui` over `ctx.viewport_rect()` (same pattern as bevy_egui's
  `side_panel` example).

Verification: `cargo build`, `cargo build --features three-mf`, and
`cargo clippy --all-targets` are clean (the one remaining warning is a pre-existing
linker `__eh_frame` note from Bevy's debug binary size). `cargo test`: 6 passed,
0 failed. Ran the binary against a test STL twice (~15s and ~12s), no panics; logs
confirm `Model request complete` and `Mesh imported and normalized`.

Confirmed interactively on 2026-08-14 via `cargo run` on macOS 26.2 / Apple M4 Pro
(Metal): window opened, a GLB was loaded through the File menu
(`Scene ready and normalized`, total 13415 ms), and the app exited cleanly. No panics
or errors in the session log. Automated screenshot capture was not possible in this
environment (`screencapture` reports `could not create image from display` — no
screen-recording permission), so the check was done by the user.

Remaining issues: `bevy_egui` logs a benign startup warning that bindless textures
are unsupported on Metal and it is disabling bindless mode (bevyengine/bevy#18149).

## Unreleased — Auto-scale STL to consistent view size

- STL has no unit metadata; raw coordinates can be millimeters, inches, or meters.
- `compute_fit_scale` auto-scales imported STL vertices so the model's largest bounding-box dimension becomes 3.0 Bevy units.
- Fixes models appearing 1000× too large (mm treated as m) and grid moiré caused by extreme scale mismatch.
- Updated `converts_stl_z_up_to_bevy_y_up` test for scaled coordinate output.
- Added screenshot to README.

## 759ee20 — Add STL support, fix coordinate system, improve lighting and grid

## 759ee20 — Add STL support, fix coordinate system, improve lighting and grid

- STL loading (ASCII + binary) via `stl_io`, integrated into the existing generic mesh-import pipeline shared with 3MF.
- STL Z-up → Bevy Y-up conversion `[x, z, -y]` (proper rotation, determinant = +1) so faces render outward and the model stands upright.
- Emptied/empty-mesh STL returns `"STL file contains no triangles"`.
- Generic async-import channel (`PendingMeshImport` / `ImportMeshTask`) replaces the previous 3MF-only poller.
- Two directional lights: main key light 25k lux (shadows on), fill light 12k lux (shadows off), plus ambient light at 1500 brightness with slight blue tint — eliminates pitch-black shadows.
- Ground grid `fadeout_distance` increased from 100 to 10 000 so it stays visible for all model sizes.
- New tests: `imports_ascii_stl`, `imports_binary_stl`, `converts_stl_z_up_to_bevy_y_up`, `rejects_empty_stl`.

## 0079816 — Improve model loading and framing

- `ModelRequest` resource with generation counter, file name, loading status, and stages.
- Generation-based model replacement: each new request bumps generation, stale entities and pending async tasks are despawned.
- Loading UI: centered overlay with `Loading…` / `Failed to load`; stages: `LoadingAsset` → `SpawningScene` → `FramingModel`.
- Window title updated to `rust-model-viewer - <file name>`.
- `sync_window_title_and_loading_ui` system in both `Update` and `PostStartup` to handle deferred commands at startup.
- Asset load-failure detection for GLB/GLTF/OBJ.
- Unified AABB-based centering + ground placement for GLB/GLTF/OBJ and 3MF.
- Automatic camera framing via bounding-sphere radius.
- Infinite grid (editor-style, colored axes, major/minor lines) via `bevy_dev_tools`.
- Import logging with elapsed time per stage.

## e3b77f2 — Initial Rust 3D model viewer

- Bevy 0.19 app with OBJ, GLB/GLTF loading, PanOrbit camera.
- Drag-and-drop + `O` file picker via `rfd`.
- Optional 3MF support (`threemf`) with unit conversion, build items, components, transforms, index validation, flat normals, and BambuStudio split-model workaround.
- Startup model from CLI argument.
- Basic directional light and camera.
