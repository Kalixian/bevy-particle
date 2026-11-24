use bevy::{prelude::*, window::PrimaryWindow};
use bevy_particle::AttractorTarget;

use crate::{
    camera::SceneCamera,
    input::{CaptureSystems, InputCapture},
};

/// Points [`AttractorTarget`] at the cursor while the left mouse button is held, unless it was
/// pressed on the UI.
pub struct CursorPlugin;

impl Plugin for CursorPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, update_target.after(CaptureSystems));
    }
}

fn update_target(
    buttons: Res<ButtonInput<MouseButton>>,
    capture: Res<InputCapture>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<SceneCamera>>,
    mut target: ResMut<AttractorTarget>,
    mut attracting: Local<bool>,
) {
    // Where the press started decides, so a drag can cross the UI's edge either way.
    if buttons.just_pressed(MouseButton::Left) {
        *attracting = !capture.pointer;
    }
    *attracting &= buttons.pressed(MouseButton::Left);

    let (camera, camera_transform) = *camera;
    target.0 = attracting
        .then(|| window.cursor_position())
        .flatten()
        .and_then(|cursor| camera.viewport_to_world_2d(camera_transform, cursor).ok());
}
