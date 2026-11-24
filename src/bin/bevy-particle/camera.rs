use bevy::{
    core_pipeline::tonemapping::{DebandDither, Tonemapping},
    input::mouse::{AccumulatedMouseScroll, MouseScrollUnit},
    post_process::bloom::Bloom,
    prelude::*,
    window::PrimaryWindow,
};

use crate::input::{CaptureSystems, InputCapture};

/// The camera that shows the scene.
#[derive(Component)]
pub struct SceneCamera;

/// Undoes panning and zooming.
#[derive(Message)]
pub struct ResetView;

/// How much closer one notch of the scroll wheel gets.
const ZOOM_PER_NOTCH: f32 = 1.15;
/// What a notch is for devices that scroll by pixels.
const PIXELS_PER_NOTCH: f32 = 100.0;
/// World units per logical pixel.
const SCALE_RANGE: std::ops::RangeInclusive<f32> = 0.05..=20.0;

/// Spawns the 2D HDR camera with bloom. Dragging with the middle mouse button pans it, and the
/// scroll wheel zooms.
pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ResetView>().add_systems(Startup, spawn_camera).add_systems(
            Update,
            (reset_view, pan_camera, zoom_camera).chain().after(CaptureSystems),
        );
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

pub fn reset_view(
    mut resets: MessageReader<ResetView>,
    camera: Single<(&mut Transform, &mut Projection), With<SceneCamera>>,
) {
    if resets.read().count() == 0 {
        return;
    }
    let (mut transform, mut projection) = camera.into_inner();
    *transform = Transform::default();
    if let Projection::Orthographic(projection) = &mut *projection {
        projection.scale = 1.0;
    }
}

/// Keeps what's under the cursor there while the middle mouse button is held, unless it was
/// pressed on the UI.
fn pan_camera(
    buttons: Res<ButtonInput<MouseButton>>,
    capture: Res<InputCapture>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform, &mut Transform), With<SceneCamera>>,
    mut last_cursor: Local<Option<Vec2>>,
) {
    if !buttons.pressed(MouseButton::Middle) {
        *last_cursor = None;
        return;
    }
    // Outside the window there's no position; carry on from the last one when it's back.
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    if buttons.just_pressed(MouseButton::Middle) {
        *last_cursor = (!capture.pointer).then_some(cursor);
    }
    let Some(last) = *last_cursor else {
        return;
    };
    let (camera, camera_transform, mut transform) = camera.into_inner();
    let world = |cursor| camera.viewport_to_world_2d(camera_transform, cursor);
    if cursor != last
        && let (Ok(from), Ok(to)) = (world(last), world(cursor))
    {
        transform.translation += (from - to).extend(0.0);
        *last_cursor = Some(cursor);
    }
}

/// Zooms around the cursor, unless it's on the UI.
fn zoom_camera(
    scroll: Res<AccumulatedMouseScroll>,
    capture: Res<InputCapture>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &mut Transform, &mut Projection), With<SceneCamera>>,
) {
    let notches = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / PIXELS_PER_NOTCH,
    };
    if notches == 0.0 || capture.scroll {
        return;
    }
    let (camera, mut transform, mut projection) = camera.into_inner();
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    // With this frame's panning, which `GlobalTransform` doesn't have yet.
    let camera_transform = GlobalTransform::from(*transform);
    let Ok(anchor) = camera.viewport_to_world_2d(&camera_transform, cursor) else {
        return;
    };
    if let Projection::Orthographic(projection) = &mut *projection {
        let position = transform.translation.truncate();
        let (scale, position) = zoom(projection.scale, position, anchor, notches);
        projection.scale = scale;
        transform.translation = position.extend(transform.translation.z);
    }
}

/// The scale and position of a camera after zooming in by some notches, with `anchor` staying in
/// place on the screen.
fn zoom(scale: f32, position: Vec2, anchor: Vec2, notches: f32) -> (f32, Vec2) {
    let (min, max) = (*SCALE_RANGE.start(), *SCALE_RANGE.end());
    let new_scale = (scale * ZOOM_PER_NOTCH.powf(-notches)).clamp(min, max);
    (new_scale, anchor + (position - anchor) * (new_scale / scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zooming_keeps_the_anchor_in_place_on_screen() {
        let (position, anchor) = (Vec2::new(10.0, 20.0), Vec2::new(110.0, -30.0));
        for notches in [1.0, -3.0, 0.2] {
            let (scale, moved) = zoom(2.0, position, anchor, notches);
            let (before, after) = ((anchor - position) / 2.0, (anchor - moved) / scale);
            assert!(before.distance(after) < 1e-3, "{before} became {after}");
        }
        // Back to where it was.
        let (scale, moved) = zoom(2.0, position, anchor, 3.0);
        let (scale, moved) = zoom(scale, moved, anchor, -3.0);
        assert!((scale - 2.0).abs() < 1e-4 && moved.distance(position) < 1e-3);
    }

    #[test]
    fn zooming_stops_at_the_limits() {
        for (notches, limit) in [(500.0, *SCALE_RANGE.start()), (-500.0, *SCALE_RANGE.end())] {
            let (scale, position) = zoom(1.0, Vec2::ZERO, Vec2::ONE, notches);
            assert_eq!(scale, limit);
            // And doesn't move from there.
            assert_eq!(zoom(scale, position, Vec2::ONE, notches), (scale, position));
        }
    }
}
