# Rust Model Viewer

A small native model viewer built mostly from existing Rust crates instead of a custom renderer.

## Stack

- `bevy`: scene, PBR rendering, GLB/GLTF loading, lighting, and the `wgpu` backend
- `bevy_obj`: OBJ loading
- `bevy_panorbit_camera`: orbit, pan, and zoom controls
- `rfd`: native file picker
- `stl_io`: ASCII and binary STL parsing
- `threemf`: optional 3MF package parsing

## Run

```bash
cargo run
```

Open a GLB, GLTF, OBJ, or STL by dropping it into the window or pressing `O`.
You can also pass a model path when starting the viewer:

```bash
cargo run --features three-mf -- /path/to/model.glb
```

Camera controls:

- Left mouse drag: orbit
- Right mouse drag: pan
- Mouse wheel: zoom

Opening or dropping a supported file replaces the current model immediately. The current file name is shown in the native window title.

The viewer displays an editor-style ground grid. Imported GLB, GLTF, OBJ, STL, and 3MF geometry is centered on the grid by its overall bounding box, placed on the ground plane, and automatically framed by the camera.

## Experimental 3MF Support

```bash
cargo run --features three-mf
```

The 3MF importer supports triangle meshes, build items, component instances, declared units, and object transforms. It does not import 3MF materials, textures, or extensions.

## Architecture

The application does not implement a rendering engine. Bevy owns the render graph, PBR materials, scene system, and GPU resource management. Bevy uses `wgpu` under the hood, while the viewer only wires together existing loading and camera plugins.
