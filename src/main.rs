use bevy::{
    core_pipeline::tonemapping::{DebandDither, Tonemapping},
    post_process::bloom::{Bloom, BloomCompositeMode},
    prelude::*,
};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_systems(Startup, setup)
        .add_systems(Update, update_particles)
        .run();
}


fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    asset_server: Res<AssetServer>,
) {
    commands.spawn((
        Camera2d,
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        Tonemapping::TonyMcMapface, // 1. Using a tonemapper that desaturates to white is recommended
        Bloom::default(),           // 2. Enable bloom for the camera
        DebandDither::Enabled,      // Optional: bloom causes gradients which cause banding
    ));

    // Circle mesh
    for i in 0..100 {
        let x: f32 = rand::random_range(-200.0..200.0);
        let y: f32 = rand::random_range(-200.0..200.0);
        commands.spawn((
            Mesh2d(meshes.add(Circle::new(3.))),
            // 3. Put something bright in a dark environment to see the effect
            MeshMaterial2d(materials.add(Color::srgb(7.5, 0.0, 7.5))),
            Transform::from_translation(Vec3::new(x, y, 0.)),
        ));
    }
}

fn update_particles(
    time: Res<Time>,
    mut query: Query<&mut Transform, With<Mesh2d>>,
) {
    for mut transform in &mut query {
        transform.translation.x += 10.0 * time.delta_secs();
    }
}


