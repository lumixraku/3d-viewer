use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use bevy::{asset::RenderAssetUsages, mesh::Indices, render::render_resource::PrimitiveTopology};
use bevy::{
    asset::{AssetPath, AssetPlugin, LoadState, RecursiveDependencyLoadState, UnapprovedPathMode},
    camera::{primitives::Aabb, visibility::VisibilitySystems},
    dev_tools::infinite_grid::{InfiniteGrid, InfiniteGridPlugin, InfiniteGridSettings},
    gltf::GltfAssetLabel,
    prelude::*,
    tasks::{
        AsyncComputeTaskPool, Task,
        futures_lite::future::{block_on, poll_once},
    },
    transform::TransformSystems,
    window::{FileDragAndDrop, PrimaryWindow},
    world_serialization::WorldInstanceReady,
};
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};
use bevy_obj::ObjPlugin;
use bevy_panorbit_camera::{EguiFocusIncludesHover, PanOrbitCamera, PanOrbitCameraPlugin};
use rfd::AsyncFileDialog;

/// Sent when the user asks to open a file, either from the menu or the `O` shortcut.
#[derive(Message)]
struct OpenFileRequested;

#[derive(Component)]
struct LoadedModel;

#[derive(Component)]
struct ModelGeneration(u64);

#[derive(Resource, Default)]
struct ModelRequest {
    generation: u64,
    file_name: Option<String>,
    status: LoadingStatus,
    started_at: Option<Instant>,
    stage: LoadingStage,
}

#[derive(Default, PartialEq, Eq)]
enum LoadingStatus {
    #[default]
    Idle,
    Loading,
    Failed,
}

#[derive(Default, PartialEq, Eq)]
enum LoadingStage {
    #[default]
    LoadingAsset,
    SpawningScene,
    FramingModel,
}

#[derive(Component)]
struct PendingModelNormalization;

type PendingModelRoot = (
    With<WorldAssetRoot>,
    With<LoadedModel>,
    With<PendingModelNormalization>,
);

#[derive(Component)]
struct PickFileTask(Task<Option<PathBuf>>);

#[derive(Component)]
struct PendingMeshImport;

#[derive(Component)]
struct ImportMeshTask {
    generation: u64,
    task: Task<Result<Vec<ImportedMesh>, String>>,
}

struct ImportedMesh {
    name: String,
    positions: Vec<[f32; 3]>,
    indices: Vec<u32>,
}

#[derive(Clone, Copy)]
struct ModelBounds {
    min: Vec3,
    max: Vec3,
}

impl ModelBounds {
    fn center(self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(AssetPlugin {
            unapproved_path_mode: UnapprovedPathMode::Allow,
            ..default()
        }))
        .add_plugins(ObjPlugin)
        .add_plugins(PanOrbitCameraPlugin)
        .add_plugins(InfiniteGridPlugin)
        .add_plugins(EguiPlugin::default())
        // The menu bar is a Panel, so hovering it must also block camera input.
        .insert_resource(EguiFocusIncludesHover(true))
        .init_resource::<ModelRequest>()
        .add_message::<OpenFileRequested>()
        .add_systems(Startup, setup)
        .add_systems(PostStartup, load_startup_model)
        .add_observer(mark_model_ready)
        .add_systems(EguiPrimaryContextPass, draw_ui)
        .add_systems(
            Update,
            (
                request_open_on_shortcut,
                open_file_dialog,
                poll_file_dialog,
                handle_file_drop,
                detect_asset_load_failures,
                poll_mesh_import,
                sync_window_title,
            )
                .chain(),
        )
        .add_systems(PostStartup, sync_window_title.after(load_startup_model))
        .add_systems(
            PostUpdate,
            (
                discard_stale_models,
                normalize_pending_models
                    .after(discard_stale_models)
                    .after(TransformSystems::Propagate)
                    .after(VisibilitySystems::CalculateBounds),
            ),
        )
        .run();
}

fn setup(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(3.0, 2.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
        PanOrbitCamera::default(),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 25_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(3.0, 5.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(-2.0, 4.0, -2.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.92, 0.94, 1.0),
        brightness: 1500.0,
        ..default()
    });

    commands.spawn((
        InfiniteGrid,
        InfiniteGridSettings {
            x_axis_color: Color::srgb(0.78, 0.22, 0.22),
            z_axis_color: Color::srgb(0.22, 0.42, 0.82),
            minor_line_color: Color::srgba(0.28, 0.30, 0.34, 0.55),
            major_line_color: Color::srgba(0.52, 0.54, 0.58, 0.78),
            fadeout_distance: 10_000.0,
            dot_fadeout_strength: 0.25,
            scale: 1.0,
        },
    ));
}

fn sync_window_title(
    request: Res<ModelRequest>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if !request.is_changed() {
        return;
    }

    if let Ok(mut window) = windows.single_mut() {
        window.title = request.file_name.as_ref().map_or_else(
            || "rust-model-viewer".to_owned(),
            |file_name| format!("rust-model-viewer - {file_name}"),
        );
    }
}

fn status_label(request: &ModelRequest) -> Option<&'static str> {
    match request.status {
        LoadingStatus::Idle => None,
        LoadingStatus::Loading => Some(match request.stage {
            LoadingStage::LoadingAsset => "Loading model…",
            LoadingStage::SpawningScene => "Preparing scene…",
            LoadingStage::FramingModel => "Framing model…",
        }),
        LoadingStatus::Failed => Some("Failed to load"),
    }
}

fn draw_ui(
    mut contexts: EguiContexts,
    request: Res<ModelRequest>,
    mut open_requests: MessageWriter<OpenFileRequested>,
    mut exit: MessageWriter<AppExit>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let busy = request.status == LoadingStatus::Loading;

    // Panels must be shown inside a Ui; build one over the whole viewport on the
    // background layer so the 3D scene stays visible behind it.
    let mut viewport_ui = egui::Ui::new(
        ctx.clone(),
        "viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    egui::Panel::top("menu_bar").show_inside(&mut viewport_ui, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new("Open…").shortcut_text("O"))
                    .clicked()
                {
                    open_requests.write(OpenFileRequested);
                    ui.close();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    exit.write(AppExit::Success);
                    ui.close();
                }
            });
        });
    });

    if let Some(label) = status_label(&request) {
        egui::Area::new("status".into())
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if busy {
                            ui.add(egui::Spinner::new().size(20.0));
                        }
                        ui.label(label);
                    });
                });
            });
    }

    Ok(())
}

fn detect_asset_load_failures(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut request: ResMut<ModelRequest>,
    models: Query<(Entity, &WorldAssetRoot, &ModelGeneration), With<LoadedModel>>,
) {
    if request.status != LoadingStatus::Loading {
        return;
    }

    for (entity, world_root, generation) in &models {
        if generation.0 != request.generation {
            continue;
        }

        if let Some((load_state, _, recursive_state)) = asset_server.get_load_states(&world_root.0)
        {
            let fully_loaded = matches!(load_state, LoadState::Loaded)
                && matches!(recursive_state, RecursiveDependencyLoadState::Loaded);
            let failure = match (load_state, recursive_state) {
                (LoadState::Failed(error), _) => Some(error),
                (_, RecursiveDependencyLoadState::Failed(error)) => Some(error),
                _ => None,
            };

            if let Some(error) = failure {
                error!("Failed to load model generation {}: {error}", generation.0);
                request.status = LoadingStatus::Failed;
                commands.entity(entity).despawn();
            } else if fully_loaded && request.stage == LoadingStage::LoadingAsset {
                request.stage = LoadingStage::SpawningScene;
                if let Some(started_at) = request.started_at {
                    info!(
                        "Asset and dependencies loaded: generation={}, elapsed_ms={}",
                        generation.0,
                        started_at.elapsed().as_millis()
                    );
                }
            }
        }
    }
}

fn fail_current_request(request: &mut ModelRequest, generation: u64, reason: &str) {
    if request.generation != generation {
        return;
    }

    error!("Failed to load model generation {generation}: {reason}");
    request.status = LoadingStatus::Failed;
}

fn complete_current_request(request: &mut ModelRequest, generation: u64) {
    if request.generation == generation {
        if let Some(started_at) = request.started_at.take() {
            info!(
                "Model request complete: generation={generation}, total_ms={}",
                started_at.elapsed().as_millis()
            );
        }
        request.status = LoadingStatus::Idle;
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn log_accepted_request(path: &Path, generation: u64) {
    info!(
        "Accepted model request: path={}, generation={generation}",
        path.display()
    );
}

fn log_scene_ready(path: Option<&str>, generation: u64) {
    info!(
        "Scene ready and normalized: file={}, generation={generation}",
        path.unwrap_or("<unknown>")
    );
}

fn log_mesh_imported(path: Option<&str>, generation: u64) {
    info!(
        "Mesh imported and normalized: file={}, generation={generation}",
        path.unwrap_or("<unknown>")
    );
}

fn load_startup_model(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut request: ResMut<ModelRequest>,
    loaded_models: Query<Entity, With<LoadedModel>>,
    import_tasks: Query<Entity, With<PendingMeshImport>>,
) {
    let Some(path) = std::env::args_os().nth(1).map(PathBuf::from) else {
        return;
    };

    load_model(
        &mut commands,
        &asset_server,
        &path,
        &mut request,
        &loaded_models,
        &import_tasks,
    );
}

fn mark_model_ready(
    ready: On<WorldInstanceReady>,
    mut commands: Commands,
    roots: Query<(), (With<LoadedModel>, With<WorldAssetRoot>)>,
    mut request: ResMut<ModelRequest>,
) {
    if roots.get(ready.entity).is_ok() {
        request.stage = LoadingStage::FramingModel;
        if let Some(started_at) = request.started_at {
            info!(
                "Scene instantiated: generation={}, elapsed_ms={}",
                request.generation,
                started_at.elapsed().as_millis()
            );
        }
        commands
            .entity(ready.entity)
            .insert(PendingModelNormalization);
    }
}

fn discard_stale_models(
    mut commands: Commands,
    request: Res<ModelRequest>,
    models: Query<(Entity, &ModelGeneration), With<LoadedModel>>,
) {
    for (entity, generation) in &models {
        if generation.0 != request.generation {
            commands.entity(entity).despawn();
        }
    }
}

fn normalize_pending_models(
    mut commands: Commands,
    mut request: ResMut<ModelRequest>,
    mut roots: Query<(Entity, &ModelGeneration, &mut Transform), PendingModelRoot>,
    children: Query<&Children>,
    mesh_entities: Query<(), With<Mesh3d>>,
    bounds: Query<(&Aabb, &GlobalTransform)>,
    mut cameras: Query<(&Projection, &mut PanOrbitCamera), With<Camera3d>>,
) {
    for (root, generation, mut root_transform) in &mut roots {
        if generation.0 != request.generation {
            commands.entity(root).despawn();
            continue;
        }

        let Some((min, max)) = compute_world_aabb(root, &children, &mesh_entities, &bounds) else {
            continue;
        };

        let center = (min + max) * 0.5;
        let offset = Vec3::new(-center.x, -min.y, -center.z);
        root_transform.translation += offset;

        let normalized_min = min + offset;
        let normalized_max = max + offset;
        let normalized_center = center + offset;

        if let Ok((projection, mut camera)) = cameras.single_mut() {
            frame_camera_from_aabb(
                normalized_min,
                normalized_max,
                normalized_center,
                projection,
                &mut camera,
            );
        }

        commands.entity(root).remove::<PendingModelNormalization>();
        complete_current_request(&mut request, generation.0);
        log_scene_ready(request.file_name.as_deref(), generation.0);
    }
}

fn compute_world_aabb(
    root: Entity,
    children: &Query<&Children>,
    mesh_entities: &Query<(), With<Mesh3d>>,
    bounds: &Query<(&Aabb, &GlobalTransform)>,
) -> Option<(Vec3, Vec3)> {
    let entities = std::iter::once(root).chain(children.iter_descendants(root));
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut saw_mesh = false;

    for entity in entities {
        if mesh_entities.get(entity).is_err() {
            continue;
        }
        saw_mesh = true;

        let Ok((aabb, global_transform)) = bounds.get(entity) else {
            return None;
        };

        let center: Vec3 = aabb.center.into();
        let half_extents: Vec3 = aabb.half_extents.into();
        for sx in [-1.0, 1.0] {
            for sy in [-1.0, 1.0] {
                for sz in [-1.0, 1.0] {
                    let local_corner = center + half_extents * Vec3::new(sx, sy, sz);
                    let world_corner = global_transform.to_matrix().transform_point3(local_corner);
                    min = min.min(world_corner);
                    max = max.max(world_corner);
                }
            }
        }
    }

    (saw_mesh && min.is_finite() && max.is_finite()).then_some((min, max))
}

fn request_open_on_shortcut(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut open_requests: MessageWriter<OpenFileRequested>,
) {
    if keyboard.just_pressed(KeyCode::KeyO) {
        open_requests.write(OpenFileRequested);
    }
}

fn open_file_dialog(
    mut open_requests: MessageReader<OpenFileRequested>,
    pending_tasks: Query<(), With<PickFileTask>>,
    mut commands: Commands,
) {
    // Collapse repeats within a frame: one dialog at a time.
    if open_requests.read().count() == 0 || !pending_tasks.is_empty() {
        return;
    }

    let task = AsyncComputeTaskPool::get().spawn(async {
        AsyncFileDialog::new()
            .set_title("Open 3D model")
            .add_filter("3D models", supported_extensions())
            .pick_file()
            .await
            .map(|file| file.path().to_owned())
    });

    commands.spawn(PickFileTask(task));
}

fn supported_extensions() -> &'static [&'static str] {
    #[cfg(feature = "three-mf")]
    {
        &["glb", "gltf", "obj", "stl", "3mf"]
    }

    #[cfg(not(feature = "three-mf"))]
    {
        &["glb", "gltf", "obj", "stl"]
    }
}

fn poll_file_dialog(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut request: ResMut<ModelRequest>,
    mut tasks: Query<(Entity, &mut PickFileTask)>,
    loaded_models: Query<Entity, With<LoadedModel>>,
    import_tasks: Query<Entity, With<PendingMeshImport>>,
) {
    for (entity, mut task) in &mut tasks {
        let Some(result) = block_on(poll_once(&mut task.0)) else {
            continue;
        };

        commands.entity(entity).despawn();

        if let Some(path) = result {
            load_model(
                &mut commands,
                &asset_server,
                &path,
                &mut request,
                &loaded_models,
                &import_tasks,
            );
        }
    }
}

fn handle_file_drop(
    mut messages: MessageReader<FileDragAndDrop>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut request: ResMut<ModelRequest>,
    loaded_models: Query<Entity, With<LoadedModel>>,
    import_tasks: Query<Entity, With<PendingMeshImport>>,
) {
    for message in messages.read() {
        let FileDragAndDrop::DroppedFile { path_buf, .. } = message else {
            continue;
        };

        load_model(
            &mut commands,
            &asset_server,
            path_buf,
            &mut request,
            &loaded_models,
            &import_tasks,
        );
    }
}

fn load_model(
    commands: &mut Commands,
    asset_server: &AssetServer,
    path: &Path,
    request: &mut ModelRequest,
    loaded_models: &Query<Entity, With<LoadedModel>>,
    import_tasks: &Query<Entity, With<PendingMeshImport>>,
) {
    let Some(extension) = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
    else {
        warn!("File has no extension: {}", path.display());
        return;
    };

    if !supported_extensions().contains(&extension.as_str()) {
        warn!("Unsupported file type: {}", path.display());
        return;
    }

    request.generation = request.generation.wrapping_add(1);
    let generation = request.generation;
    request.file_name = Some(file_name(path));
    request.status = LoadingStatus::Loading;
    request.started_at = Some(Instant::now());
    request.stage = LoadingStage::LoadingAsset;
    log_accepted_request(path, generation);

    for entity in loaded_models.iter() {
        commands.entity(entity).despawn();
    }
    for entity in import_tasks.iter() {
        commands.entity(entity).despawn();
    }

    let asset_path = AssetPath::from_path_buf(path.to_owned());

    match extension.as_str() {
        "glb" | "gltf" => {
            let scene = asset_server.load(GltfAssetLabel::Scene(0).from_asset(asset_path));
            commands.spawn((
                WorldAssetRoot(scene),
                LoadedModel,
                ModelGeneration(generation),
            ));
        }
        "obj" => {
            let scene = asset_server.load(asset_path);
            commands.spawn((
                WorldAssetRoot(scene),
                LoadedModel,
                ModelGeneration(generation),
            ));
        }
        "stl" => {
            let path = path.to_owned();
            let task = AsyncComputeTaskPool::get().spawn(async move { import_stl(&path) });
            commands.spawn((PendingMeshImport, ImportMeshTask { generation, task }));
        }
        #[cfg(feature = "three-mf")]
        "3mf" => {
            let path = path.to_owned();
            let task = AsyncComputeTaskPool::get().spawn(async move { import_3mf(&path) });
            commands.spawn((PendingMeshImport, ImportMeshTask { generation, task }));
        }
        _ => unreachable!("supported extension was checked above"),
    }
}

fn poll_mesh_import(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut request: ResMut<ModelRequest>,
    mut cameras: Query<(&Projection, &mut PanOrbitCamera), With<Camera3d>>,
    mut tasks: Query<(Entity, &mut ImportMeshTask)>,
) {
    for (entity, mut task) in &mut tasks {
        let generation = task.generation;
        let Some(result) = block_on(poll_once(&mut task.task)) else {
            continue;
        };

        commands.entity(entity).despawn();

        if generation != request.generation {
            continue;
        }

        match result {
            Ok(mut imported_meshes) => {
                let Some(bounds) = normalize_imported_meshes(&mut imported_meshes) else {
                    fail_current_request(&mut request, generation, "model contains no vertices");
                    continue;
                };

                if let Ok((projection, mut camera)) = cameras.single_mut() {
                    frame_camera_from_aabb(
                        bounds.min,
                        bounds.max,
                        bounds.center(),
                        projection,
                        &mut camera,
                    );
                }

                let material = materials.add(StandardMaterial {
                    base_color: Color::srgb(0.72, 0.74, 0.78),
                    perceptual_roughness: 0.65,
                    ..default()
                });

                let imported_meshes = imported_meshes
                    .into_iter()
                    .map(|imported| {
                        make_mesh(imported.positions, imported.indices)
                            .map(|mesh| (imported.name, mesh))
                    })
                    .collect::<Result<Vec<_>, _>>();
                let imported_meshes = match imported_meshes {
                    Ok(imported_meshes) => imported_meshes,
                    Err(error) => {
                        fail_current_request(&mut request, generation, &error);
                        continue;
                    }
                };

                for (name, mesh) in imported_meshes {
                    commands.spawn((
                        Name::new(name),
                        Mesh3d(meshes.add(mesh)),
                        MeshMaterial3d(material.clone()),
                        LoadedModel,
                        ModelGeneration(generation),
                    ));
                }

                complete_current_request(&mut request, generation);
                log_mesh_imported(request.file_name.as_deref(), generation);
            }
            Err(error) => fail_current_request(&mut request, generation, &error),
        }
    }
}

fn normalize_imported_meshes(meshes: &mut [ImportedMesh]) -> Option<ModelBounds> {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);

    for mesh in meshes.iter() {
        for position in &mesh.positions {
            let position = Vec3::from_array(*position);
            min = min.min(position);
            max = max.max(position);
        }
    }

    if !min.is_finite() || !max.is_finite() {
        return None;
    }

    let offset = Vec3::new(-(min.x + max.x) * 0.5, -min.y, -(min.z + max.z) * 0.5);
    for mesh in meshes {
        for position in &mut mesh.positions {
            let normalized = Vec3::from_array(*position) + offset;
            *position = normalized.to_array();
        }
    }

    Some(ModelBounds {
        min: min + offset,
        max: max + offset,
    })
}

fn frame_camera_from_aabb(
    min: Vec3,
    max: Vec3,
    center: Vec3,
    projection: &Projection,
    camera: &mut PanOrbitCamera,
) {
    let half_extents = (max - min) * 0.5;
    let sphere_radius = half_extents.length().max(0.1);
    let radius = match projection {
        Projection::Perspective(perspective) => {
            let vertical_half_fov = perspective.fov * 0.5;
            let horizontal_half_fov = (vertical_half_fov.tan() * perspective.aspect_ratio).atan();
            let limiting_half_fov = vertical_half_fov.min(horizontal_half_fov);
            (sphere_radius / limiting_half_fov.sin() * 1.15).max(0.1)
        }
        _ => (sphere_radius * 2.0).max(0.1),
    };

    camera.target_focus = center;
    camera.target_radius = radius;
}

fn import_stl(path: &Path) -> Result<Vec<ImportedMesh>, String> {
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    read_stl(&mut file, file_name(path))
}

fn read_stl(
    reader: &mut (impl std::io::Read + std::io::Seek),
    name: String,
) -> Result<Vec<ImportedMesh>, String> {
    let mesh = stl_io::read_stl(reader).map_err(|error| error.to_string())?;
    if mesh.vertices.is_empty() || mesh.faces.is_empty() {
        return Err("STL file contains no triangles".to_owned());
    }

    let positions: Vec<[f32; 3]> = mesh
        .vertices
        .into_iter()
        .map(|vertex| {
            let [x, y, z] = vertex.0;
            [x, z, -y]
        })
        .collect();
    let indices = mesh
        .faces
        .into_iter()
        .flat_map(|face| face.vertices)
        .map(|index| u32::try_from(index).map_err(|_| format!("vertex index {index} exceeds u32")))
        .collect::<Result<Vec<_>, _>>()?;

    let scale = compute_fit_scale(&positions);
    let positions = positions
        .into_iter()
        .map(|pos| (Vec3::from_array(pos) * scale).to_array())
        .collect();

    Ok(vec![ImportedMesh {
        name,
        positions,
        indices,
    }])
}

fn compute_fit_scale(positions: &[[f32; 3]]) -> f32 {
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for pos in positions {
        let p = Vec3::from_array(*pos);
        min = min.min(p);
        max = max.max(p);
    }
    let extent = max - min;
    let max_extent = extent.x.max(extent.y).max(extent.z);
    if max_extent > 0.0 {
        3.0 / max_extent
    } else {
        1.0
    }
}

#[cfg(feature = "three-mf")]
fn import_3mf(path: &Path) -> Result<Vec<ImportedMesh>, String> {
    use std::{collections::HashMap, fs::File};

    use threemf::model::{Object, Unit};

    let file = File::open(path).map_err(|error| error.to_string())?;
    let models = threemf::read(file).map_err(|error| error.to_string())?;
    let mut imported = Vec::new();
    let objects: HashMap<usize, &Object> = models
        .iter()
        .flat_map(|model| &model.resources.object)
        .map(|object| (object.id, object))
        .collect();

    for model in &models {
        let scale = match model.unit {
            Unit::Micron => 0.000_001,
            Unit::Millimeter => 0.001,
            Unit::Centimeter => 0.01,
            Unit::Inch => 0.0254,
            Unit::Foot => 0.3048,
            Unit::Meter => 1.0,
        };
        for item in &model.build.item {
            collect_object_meshes(
                &objects,
                item.objectid,
                item.transform.unwrap_or(IDENTITY_3MF),
                scale,
                &mut Vec::new(),
                &mut imported,
            )?;
        }
    }

    if imported.is_empty() {
        return Err("the file contains no supported triangle meshes".to_owned());
    }

    Ok(imported)
}

#[cfg(feature = "three-mf")]
const IDENTITY_3MF: [f64; 12] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];

#[cfg(feature = "three-mf")]
fn collect_object_meshes(
    objects: &std::collections::HashMap<usize, &threemf::model::Object>,
    object_id: usize,
    transform: [f64; 12],
    unit_scale: f64,
    stack: &mut Vec<usize>,
    imported: &mut Vec<ImportedMesh>,
) -> Result<(), String> {
    let object = objects
        .get(&object_id)
        .ok_or_else(|| format!("object {object_id} does not exist"))?;

    if stack.contains(&object_id) {
        return Err(format!("component cycle detected at object {object_id}"));
    }
    stack.push(object_id);

    if let Some(mesh) = &object.mesh {
        let positions = mesh
            .vertices
            .vertex
            .iter()
            .map(|vertex| {
                let [x, y, z] = transform_3mf_point(transform, [vertex.x, vertex.y, vertex.z]);
                [
                    (x * unit_scale) as f32,
                    (z * unit_scale) as f32,
                    (-y * unit_scale) as f32,
                ]
            })
            .collect();
        let indices = mesh
            .triangles
            .triangle
            .iter()
            .flat_map(|triangle| [triangle.v1, triangle.v2, triangle.v3])
            .map(|index| {
                u32::try_from(index).map_err(|_| format!("vertex index {index} exceeds u32"))
            })
            .collect::<Result<Vec<_>, _>>()?;

        imported.push(ImportedMesh {
            name: object
                .name
                .clone()
                .unwrap_or_else(|| format!("3MF object {object_id}")),
            positions,
            indices,
        });
    }

    if let Some(components) = &object.components {
        for component in &components.component {
            collect_object_meshes(
                objects,
                component.objectid,
                compose_3mf_transforms(transform, component.transform.unwrap_or(IDENTITY_3MF)),
                unit_scale,
                stack,
                imported,
            )?;
        }
    }

    stack.pop();
    Ok(())
}

#[cfg(feature = "three-mf")]
fn transform_3mf_point(transform: [f64; 12], point: [f64; 3]) -> [f64; 3] {
    [
        transform[0] * point[0] + transform[3] * point[1] + transform[6] * point[2] + transform[9],
        transform[1] * point[0] + transform[4] * point[1] + transform[7] * point[2] + transform[10],
        transform[2] * point[0] + transform[5] * point[1] + transform[8] * point[2] + transform[11],
    ]
}

#[cfg(feature = "three-mf")]
fn compose_3mf_transforms(parent: [f64; 12], child: [f64; 12]) -> [f64; 12] {
    let mut result = [0.0; 12];
    for column in 0..3 {
        for row in 0..3 {
            result[column * 3 + row] = (0..3)
                .map(|index| parent[index * 3 + row] * child[column * 3 + index])
                .sum();
        }
    }
    for row in 0..3 {
        result[9 + row] = parent[9 + row]
            + (0..3)
                .map(|index| parent[index * 3 + row] * child[9 + index])
                .sum::<f64>();
    }
    result
}

fn make_mesh(positions: Vec<[f32; 3]>, indices: Vec<u32>) -> Result<Mesh, String> {
    if !indices.len().is_multiple_of(3) {
        return Err("triangle index count is not divisible by 3".to_owned());
    }

    let vertex_count = u32::try_from(positions.len())
        .map_err(|_| "vertex count exceeds the supported range".to_owned())?;
    if let Some(index) = indices.iter().find(|index| **index >= vertex_count) {
        return Err(format!(
            "triangle index {index} exceeds vertex count {vertex_count}"
        ));
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_indices(Indices::U32(indices));
    mesh.duplicate_vertices();
    mesh.compute_flat_normals();
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_ascii_stl() {
        let source = b"solid triangle\n\
            facet normal 0 0 1\n\
              outer loop\n\
                vertex 0 0 0\n\
                vertex 1 0 0\n\
                vertex 0 1 0\n\
              endloop\n\
            endfacet\n\
            endsolid triangle\n";
        let mut reader = std::io::Cursor::new(source);

        let meshes = read_stl(&mut reader, "triangle.stl".to_owned()).unwrap();

        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0].name, "triangle.stl");
        assert_eq!(meshes[0].positions.len(), 3);
        assert_eq!(meshes[0].indices, vec![0, 1, 2]);
    }

    #[test]
    fn imports_binary_stl() {
        let mut source = vec![0; 80];
        source.extend_from_slice(&1_u32.to_le_bytes());
        for value in [
            0.0_f32, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0,
        ] {
            source.extend_from_slice(&value.to_le_bytes());
        }
        source.extend_from_slice(&0_u16.to_le_bytes());
        let mut reader = std::io::Cursor::new(source);

        let meshes = read_stl(&mut reader, "triangle.stl".to_owned()).unwrap();

        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0].positions.len(), 3);
        assert_eq!(meshes[0].indices, vec![0, 1, 2]);
    }

    #[test]
    fn converts_stl_z_up_to_bevy_y_up() {
        let source = b"solid triangle\n\
            facet normal 0 0 1\n\
              outer loop\n\
                vertex 0 0 1\n\
                vertex 1 0 1\n\
                vertex 0 1 1\n\
              endloop\n\
            endfacet\n\
            endsolid triangle\n";
        let mut reader = std::io::Cursor::new(source);

        let meshes = read_stl(&mut reader, "triangle.stl".to_owned()).unwrap();

        let positions = &meshes[0].positions;
        assert_eq!(positions[0], [0.0, 3.0, 0.0]);
        assert_eq!(positions[1], [3.0, 3.0, 0.0]);
        assert_eq!(positions[2], [0.0, 3.0, -3.0]);
    }

    #[test]
    fn rejects_empty_stl() {
        let mut reader = std::io::Cursor::new(b"");
        assert!(read_stl(&mut reader, "empty.stl".to_owned()).is_err());
    }

    #[cfg(feature = "three-mf")]
    #[test]
    fn transforms_points_using_3mf_column_vectors() {
        let transform = [
            0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 10.0, 20.0, 30.0,
        ];

        assert_eq!(
            transform_3mf_point(transform, [2.0, 3.0, 4.0]),
            [7.0, 22.0, 34.0]
        );
    }

    #[cfg(feature = "three-mf")]
    #[test]
    fn composes_parent_and_child_transforms() {
        let parent = [0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 10.0, 0.0, 0.0];
        let child = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 2.0, 0.0, 0.0];
        let point = [1.0, 0.0, 0.0];

        let composed = transform_3mf_point(compose_3mf_transforms(parent, child), point);
        let sequential = transform_3mf_point(parent, transform_3mf_point(child, point));

        assert_eq!(composed, sequential);
    }

    #[test]
    fn rejects_out_of_bounds_mesh_indices() {
        let error = make_mesh(vec![[0.0, 0.0, 0.0]], vec![0, 1, 0]).unwrap_err();

        assert!(error.contains("exceeds vertex count"));
    }

    #[test]
    fn centers_all_meshes_and_places_them_on_the_ground() {
        let mut meshes = vec![
            ImportedMesh {
                name: "left".to_owned(),
                positions: vec![[-4.0, 2.0, -1.0], [-2.0, 4.0, 1.0]],
                indices: vec![],
            },
            ImportedMesh {
                name: "right".to_owned(),
                positions: vec![[2.0, 3.0, 3.0], [6.0, 8.0, 5.0]],
                indices: vec![],
            },
        ];
        let separation_before =
            Vec3::from_array(meshes[1].positions[0]) - Vec3::from_array(meshes[0].positions[0]);

        let bounds = normalize_imported_meshes(&mut meshes).unwrap();
        let separation_after =
            Vec3::from_array(meshes[1].positions[0]) - Vec3::from_array(meshes[0].positions[0]);

        assert_eq!(bounds.min, Vec3::new(-5.0, 0.0, -3.0));
        assert_eq!(bounds.max, Vec3::new(5.0, 6.0, 3.0));
        assert_eq!(separation_before, separation_after);
    }
}
