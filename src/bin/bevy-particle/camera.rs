use bevy::{
    core_pipeline::tonemapping::{DebandDither, Tonemapping},
    post_process::bloom::Bloom,
    prelude::*,
};

/// The camera that shows the scene.
#[derive(Component)]
pub struct SceneCamera;

/// Spawns the 2D HDR camera with bloom.
pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera);
    }
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        SceneCamera,
        Camera2d,
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        // A tonemapper that desaturates to white works best with bloom.
        Tonemapping::TonyMcMapface,
        Bloom::default(),
        // Bloom produces gradients, which can band without dithering.
        DebandDither::Enabled,
    ));
}
