mod camera;
mod emitter;
mod gpu;
mod modifier;
mod particle;
mod space;

use bevy::{prelude::*, window::PrimaryWindow};

use crate::{
    emitter::{EmitterPlugin, ParticleEmitter},
    modifier::{Modifier, ModifierPlugin, Modifiers},
    space::ParticleSpace,
};

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins,
            camera::CameraPlugin,
            ModifierPlugin,
            space::SpacePlugin,
            EmitterPlugin,
            gpu::ParticleGpuPlugin,
        ))
        .add_systems(Startup, spawn_scene)
        .add_systems(Update, move_attractor_to_mouse)
        .run();
}

/// Marks an emitter whose first `PointAttractor` follows left clicks and drags.
#[derive(Component)]
struct MouseAttractor;

/// Where the stream's attractor starts, in world coordinates.
const STREAM_ATTRACTOR_START: Vec2 = Vec2::new(300.0, 150.0);
const STREAM_POSITION: Vec2 = Vec2::new(-450.0, -150.0);

fn spawn_scene(mut commands: Commands) {
    // Space-level: pulls every particle toward the center and keeps them on screen.
    let space = commands
        .spawn((
            Name::new("Space"),
            ParticleSpace {
                capacity: 1_000_000,
            },
            Modifiers::new([
                Modifier::PointAttractor {
                    offset: Vec2::ZERO,
                    strength: 300_000.0,
                    max_accel: 100.0,
                },
                Modifier::Bounds {
                    half_extents: Vec2::new(620.0, 340.0),
                    restitution: 0.5,
                },
            ]),
        ))
        .id();

    // A one-off cloud of particles that never die. Emitter-level: only the cloud swirls.
    commands.spawn((
        Name::new("Cloud"),
        ChildOf(space),
        ParticleEmitter {
            burst_count: 300_000,
            rate: 0.0,
            lifetime: None,
            spawn_half_extents: Vec2::splat(300.0),
            speed: 5.0..50.0,
            radius: 0.3..1.0,
            red: 6.0..9.0,
            green: 0.0..0.0,
            blue: 6.0..9.0,
            alpha: 0.05,
            ..default()
        },
        Modifiers::new([
            Modifier::Vortex {
                offset: Vec2::ZERO,
                strength: 3_000.0,
                max_accel: 60.0,
            },
            // Without drag the vortex keeps adding speed and flings the cloud apart.
            Modifier::Drag(0.4),
        ]),
    ));

    // A continuous stream from the left; each particle lives 3–5 seconds
    // (about 400k alive at once). Emitter-level: pulled toward a point you set with the
    // mouse; shifts cyan -> orange, fades and shrinks.
    commands.spawn((
        Name::new("Stream"),
        ChildOf(space),
        MouseAttractor,
        Transform::from_translation(STREAM_POSITION.extend(0.0)),
        ParticleEmitter {
            rate: 100_000.0,
            lifetime: Some(3.0..5.0),
            spawn_half_extents: Vec2::splat(5.0),
            speed: 120.0..180.0,
            angle: 0.3..0.6,
            radius: 0.4..0.9,
            alpha: 0.1,
            ..default()
        },
        Modifiers::new([
            Modifier::PointAttractor {
                // Offsets are relative to the emitter.
                offset: STREAM_ATTRACTOR_START - STREAM_POSITION,
                strength: 400_000.0,
                max_accel: 200.0,
            },
            Modifier::ColorOverLife {
                start: LinearRgba::new(0.1, 40.0, 120.0, 0.1).into(),
                end: LinearRgba::new(150.0, 20.0, 2.0, 0.0).into(),
            },
            Modifier::SizeOverLife {
                start: 1.0,
                end: 0.3,
            },
        ]),
    ));
}

/// While the left mouse button is down (click or drag), moves each `MouseAttractor`
/// emitter's first `PointAttractor` to the cursor.
fn move_attractor_to_mouse(
    buttons: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform)>,
    mut emitters: Query<(&GlobalTransform, &mut Modifiers), With<MouseAttractor>>,
) {
    if !buttons.pressed(MouseButton::Left) {
        return;
    }
    let (camera, camera_transform) = *camera;
    let Some(target) = window
        .cursor_position()
        .and_then(|cursor| camera.viewport_to_world_2d(camera_transform, cursor).ok())
    else {
        return;
    };

    for (emitter_transform, mut modifiers) in &mut emitters {
        let emitter_position = emitter_transform.translation().truncate();
        if let Some(Modifier::PointAttractor { offset, .. }) = modifiers
            .iter_mut()
            .find(|m| matches!(m, Modifier::PointAttractor { .. }))
        {
            *offset = target - emitter_position;
        }
    }
}
