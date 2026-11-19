use bevy::{
    core_pipeline::tonemapping::{DebandDither, Tonemapping},
    post_process::bloom::{Bloom, BloomCompositeMode},
    prelude::*,
};

#[derive(Component)]
struct Velocity(Vec2);

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

    let count = 100;
    // Circle mesh
    for _i in 0..count{
        let x: f32 = rand::random_range(-200.0..200.0);
        let y: f32 = rand::random_range(-200.0..200.0);
        let angle: f32 = rand::random_range(0.0..std::f32::consts::TAU);
        let speed: f32 = rand::random_range(5.0..50.0);
        let red: f32 = rand::random_range(6.0..9.0);
        let blue: f32 = rand::random_range(6.0..9.0);

        let velocity = Velocity(Vec2::new(angle.cos() * speed, angle.sin() * speed));

        commands.spawn((
            Mesh2d(meshes.add(Circle::new(3.))),
            // 3. Put something bright in a dark environment to see the effect
            MeshMaterial2d(materials.add(Color::srgb(red, 0.0, blue))),
            Transform::from_translation(Vec3::new(x, y, 0.)),
            velocity,
        ));
    }
}

fn update_particles(
    time: Res<Time>,
    mut query: Query<(&mut Transform, &mut Velocity), With<Mesh2d>>,
) {
    for (mut transform, mut velocity) in &mut query {
        transform.translation.x += velocity.0.x * time.delta_secs();
        transform.translation.y += velocity.0.y * time.delta_secs();

        let distance_from_center = transform.translation.length();
        let pull_max: f32 = 50.0;
        let mut pull_strength: f32 = 0.0;
        if distance_from_center > 0.0 {
            pull_strength = 1.0/(10.0 * distance_from_center * distance_from_center) * pull_max;
        }
        let normalized_direction = -transform.translation.truncate().normalize_or_zero();
        velocity.0 += normalized_direction * pull_strength * time.delta_secs();
    }
}


