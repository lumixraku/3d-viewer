# Progress

## Unreleased — Auto-scale STL to consistent view size

- STL has no unit metadata; raw coordinates can be millimeters, inches, or meters.
- `compute_fit_scale` auto-scales imported STL vertices so the model's largest bounding-box dimension becomes 3.0 Bevy units.
- Fixes models appearing 1000× too large (mm treated as m) and grid moiré caused by extreme scale mismatch.
- Updated `converts_stl_z_up_to_bevy_y_up` test for scaled coordinate output.

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
