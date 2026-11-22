mod camera;
mod emitter;
mod gpu;
mod gravity;
mod modifier;
mod particle;
#[cfg(feature = "profile")]
mod profiling;
mod space;
mod spread;

use bevy::prelude::*;

use crate::{
    emitter::{EmitterPlugin, ParticleEmitter},
    modifier::{Modifier, ModifierPlugin, Modifiers, MouseAttractor},
    gravity::{Gravity, GravityMethod},
    space::ParticleSpace,
    spread::{Shape, Spread},
};

fn main() {
    let mut app = App::new();
    app.add_plugins((
        default_plugins(),
        camera::CameraPlugin,
        ModifierPlugin,
        space::SpacePlugin,
        EmitterPlugin,
        gpu::ParticleGpuPlugin,
    ))
    .add_systems(Startup, spawn_scene)
    .add_systems(Update, reset_scene_on_r);

    #[cfg(feature = "profile")]
    app.add_plugins(profiling::ProfilingPlugin);

    app.run();
}

fn default_plugins() -> impl PluginGroup {
    // Profiling measures uncapped frame times, so vsync goes off.
    #[cfg(feature = "profile")]
    return DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            present_mode: bevy::window::PresentMode::AutoNoVsync,
            ..default()
        }),
        ..default()
    });
    #[cfg(not(feature = "profile"))]
    DefaultPlugins
}

/// Where the stream's attractor starts, in world coordinates.
const STREAM_ATTRACTOR_START: Vec2 = Vec2::new(300.0, 150.0);
const STREAM_POSITION: Vec2 = Vec2::new(-450.0, -150.0);

/// Particles taking part in gravity. Barnes-Hut handles this easily; brute force
/// (`GravityMethod::BruteForce`) costs the square of it and slows to a crawl.
const GRAVITY_PARTICLES: u32 = 262_144;

fn spawn_scene(mut commands: Commands) {
    // Space-level: pulls every particle toward the center and keeps them on screen.
    let space = commands
        .spawn((
            Name::new("Space"),
            ParticleSpace {
                capacity: 1_000_000,
            },
            Gravity {
                strength: 15.0,
                softening: 23.0,
                method: GravityMethod::BarnesHut,
                theta: 0.5,
            },
            // Space-level: while the left mouse button is held, everything is pulled toward
            // the cursor.
            MouseAttractor {
                strength: 2_000_000.0,
                max_accel: 500.0,
            },
            // Modifiers::new([
            //     Modifier::PointAttractor {
            //         offset: Vec2::ZERO,
            //         strength: 300_000.0,
            //         max_accel: 100.0,
            //     },
            //     Modifier::Bounds {
            //         half_extents: Vec2::new(3620.0, 3340.0),
            //         restitution: 0.5,
            //     },
            // ]),
        ))
        .id();

    // A one-off cloud of particles that never die. Emitter-level: only the cloud swirls.
    // commands.spawn((
    //     Name::new("Cloud"),
    //     ChildOf(space),
    //     ParticleEmitter {
    //         burst_count: 300_000,
    //         rate: 0.0,
    //         lifetime: None,
    //         spawn_half_extents: Vec2::splat(300.0),
    //         speed: (5.0..50.0).into(),
    //         radius: (0.3..1.0).into(),
    //         red: (6.0..9.0).into(),
    //         green: (0.0..0.0).into(),
    //         blue: (6.0..9.0).into(),
    //         alpha: 0.05,
    //         ..default()
    //     },
    //     Modifiers::new([
    //         Modifier::Vortex {
    //             offset: Vec2::ZERO,
    //             strength: 3_000.0,
    //             max_accel: 60.0,
    //         },
    //         // Without drag the vortex keeps adding speed and flings the cloud apart.
    //         Modifier::Drag(0.4),
    //     ]),
    // ));

    // A self-gravitating cluster: these particles all attract each other (Barnes-Hut), so
    // the cloud collapses into clumps. Everything else ignores gravity.
    commands.spawn((
        Name::new("Cluster"),
        ChildOf(space),
        ParticleEmitter {
            burst_count: GRAVITY_PARTICLES,
            rate: 0.0,
            lifetime: None,
            spawn_half_extents: Vec2::splat(300.0),
            speed: (0.0..10.0).into(),
            radius: Spread::new(0.4..3.6, Shape::Falloff(36.0)),
            red: (6.0..9.0).into(),
            green: (3.0..5.0).into(),
            blue: (0.0..0.5).into(),
            alpha: 0.25,
            gravity: true,
            mass_per_radius: 6.0,
            ..default()
        },
        // Modifiers::new([
        //     Modifier::Vortex {
        //         offset: Vec2::ZERO,
        //         strength:  1500.0,
        //         max_accel:  80.0,
        //     },
        //     Modifier::Drag(0.04),
        // ]),
    ));

    // A continuous stream from the left; each particle lives 3-5 seconds
    // (about 400k alive at once). Emitter-level: pulled toward a fixed point, and toward the
    // cursor while the mouse is held; shifts cyan -> orange, fades and shrinks.
    // commands.spawn((
    //     Name::new("Stream"),
    //     ChildOf(space),
    //     MouseAttractor {
    //         strength: 400_000.0,
    //         max_accel: 200.0,
    //     },
    //     Transform::from_translation(STREAM_POSITION.extend(0.0)),
    //     ParticleEmitter {
    //         rate: 100_000.0,
    //         lifetime: Some((3.0..5.0).into()),
    //         spawn_half_extents: Vec2::splat(5.0),
    //         speed: (120.0..180.0).into(),
    //         angle: (0.3..0.6).into(),
    //         radius: (0.4..0.9).into(),
    //         alpha: 0.1,
    //         ..default()
    //     },
    //     Modifiers::new([
    //         Modifier::PointAttractor {
    //             // Offsets are relative to the emitter.
    //             offset: STREAM_ATTRACTOR_START - STREAM_POSITION,
    //             strength: 400_000.0,
    //             max_accel: 200.0,
    //         },
    //         Modifier::ColorOverLife {
    //             start: LinearRgba::new(0.1, 40.0, 120.0, 0.1).into(),
    //             end: LinearRgba::new(150.0, 20.0, 2.0, 0.0).into(),
    //         },
    //         Modifier::SizeOverLife {
    //             start: 1.0,
    //             end: 0.3,
    //         },
    //     ]),
    // ));
}

/// `R` despawns every particle space (with its emitters and particles) and spawns the scene
/// again, exactly as at startup.
fn reset_scene_on_r(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    spaces: Query<Entity, With<ParticleSpace>>,
) {
    if !keys.just_pressed(KeyCode::KeyR) {
        return;
    }
    for space in &spaces {
        // Despawning a space despawns its child emitters too.
        commands.entity(space).despawn();
    }
    spawn_scene(commands);
}

