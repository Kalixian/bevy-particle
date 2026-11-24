use bevy::{prelude::*, window::PrimaryWindow};
use bevy_particle::AttractorTarget;

/// Points [`AttractorTarget`] at the cursor while the left mouse button is held.
pub struct CursorPlugin;

impl Plugin for CursorPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, update_target);
    }
}

fn update_target(
    buttons: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera2d>>,
    mut target: ResMut<AttractorTarget>,
) {
    let (camera, camera_transform) = *camera;
    target.0 = buttons
        .pressed(MouseButton::Left)
        .then(|| window.cursor_position())
        .flatten()
        .and_then(|cursor| camera.viewport_to_world_2d(camera_transform, cursor).ok());
}
