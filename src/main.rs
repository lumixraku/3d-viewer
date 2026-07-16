use std::path::{Path, PathBuf};

#[cfg(feature = "three-mf")]
use bevy::{asset::RenderAssetUsages, mesh::Indices, render::render_resource::PrimitiveTopology};
use bevy::{
    asset::{AssetPath, AssetPlugin, UnapprovedPathMode},
    gltf::GltfAssetLabel,
    prelude::*,
    tasks::{
        AsyncComputeTaskPool, Task,
        futures_lite::future::{block_on, poll_once},
    },
    window::FileDragAndDrop,
};
use bevy_obj::ObjPlugin;
use bevy_panorbit_camera::{PanOrbitCamera, PanOrbitCameraPlugin};
use rfd::AsyncFileDialog;

#[derive(Component)]
struct LoadedModel;

#[derive(Component)]
struct PickFileTask(Task<Option<PathBuf>>);

#[cfg(feature = "three-mf")]
#[derive(Component)]
struct Import3mfTask(Task<Result<Vec<ImportedMesh>, String>>);

#[cfg(feature = "three-mf")]
struct ImportedMesh {
    name: String,
    positions: Vec<[f32; 3]>,
    indices: Vec<u32>,
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(AssetPlugin {
            unapproved_path_mode: UnapprovedPathMode::Allow,
            ..default()
        }))
        .add_plugins(ObjPlugin)
        .add_plugins(PanOrbitCameraPlugin)
        .add_systems(Startup, (setup, load_startup_model))
        .add_systems(
            Update,
            (open_file_dialog, poll_file_dialog, handle_file_drop),
        )
        .add_systems(Update, poll_3mf_import.run_if(feature_enabled_3mf))
        .run();
}

fn feature_enabled_3mf() -> bool {
    cfg!(feature = "three-mf")
}

fn setup(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(3.0, 2.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
        PanOrbitCamera::default(),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(3.0, 5.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 100.0,
        ..default()
    });
}

fn load_startup_model(mut commands: Commands, asset_server: Res<AssetServer>) {
    let Some(path) = std::env::args_os().nth(1).map(PathBuf::from) else {
        return;
    };

    load_model(&mut commands, &asset_server, &path);
}

fn open_file_dialog(
    keyboard: Res<ButtonInput<KeyCode>>,
    pending_tasks: Query<(), With<PickFileTask>>,
    mut commands: Commands,
) {
    if !keyboard.just_pressed(KeyCode::KeyO) || !pending_tasks.is_empty() {
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
        &["glb", "gltf", "obj", "3mf"]
    }

    #[cfg(not(feature = "three-mf"))]
    {
        &["glb", "gltf", "obj"]
    }
}

fn poll_file_dialog(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut tasks: Query<(Entity, &mut PickFileTask)>,
) {
    for (entity, mut task) in &mut tasks {
        let Some(result) = block_on(poll_once(&mut task.0)) else {
            continue;
        };

        commands.entity(entity).despawn();

        if let Some(path) = result {
            load_model(&mut commands, &asset_server, &path);
        }
    }
}

fn handle_file_drop(
    mut messages: MessageReader<FileDragAndDrop>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
) {
    for message in messages.read() {
        let FileDragAndDrop::DroppedFile { path_buf, .. } = message else {
            continue;
        };

        load_model(&mut commands, &asset_server, path_buf);
    }
}

fn load_model(commands: &mut Commands, asset_server: &AssetServer, path: &Path) {
    let Some(extension) = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
    else {
        warn!("File has no extension: {}", path.display());
        return;
    };

    let asset_path = AssetPath::from_path_buf(path.to_owned());

    match extension.as_str() {
        "glb" | "gltf" => {
            let scene = asset_server.load(GltfAssetLabel::Scene(0).from_asset(asset_path));
            commands.spawn((WorldAssetRoot(scene), LoadedModel));
        }
        "obj" => {
            let scene = asset_server.load(asset_path);
            commands.spawn((WorldAssetRoot(scene), LoadedModel));
        }
        #[cfg(feature = "three-mf")]
        "3mf" => {
            let path = path.to_owned();
            let task = AsyncComputeTaskPool::get().spawn(async move { import_3mf(&path) });
            commands.spawn(Import3mfTask(task));
        }
        _ => warn!("Unsupported file type: {}", path.display()),
    }
}

#[cfg(feature = "three-mf")]
fn poll_3mf_import(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut tasks: Query<(Entity, &mut Import3mfTask)>,
) {
    for (entity, mut task) in &mut tasks {
        let Some(result) = block_on(poll_once(&mut task.0)) else {
            continue;
        };

        commands.entity(entity).despawn();

        match result {
            Ok(imported_meshes) => {
                let material = materials.add(StandardMaterial {
                    base_color: Color::srgb(0.72, 0.74, 0.78),
                    perceptual_roughness: 0.65,
                    ..default()
                });

                for imported in imported_meshes {
                    match make_mesh(imported.positions, imported.indices) {
                        Ok(mesh) => {
                            commands.spawn((
                                Name::new(imported.name),
                                Mesh3d(meshes.add(mesh)),
                                MeshMaterial3d(material.clone()),
                                LoadedModel,
                            ));
                        }
                        Err(error) => error!("Failed to create 3MF mesh: {error}"),
                    }
                }
            }
            Err(error) => error!("Failed to import 3MF: {error}"),
        }
    }
}

#[cfg(not(feature = "three-mf"))]
fn poll_3mf_import() {}

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

#[cfg(feature = "three-mf")]
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

#[cfg(all(test, feature = "three-mf"))]
mod tests {
    use super::*;

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
}
