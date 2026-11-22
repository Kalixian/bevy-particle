mod camera;
mod emitter;
mod particle;
mod space;

use bevy::prelude::*;

use crate::{
    emitter::ParticleEmitter,
    space::{CenterPull, ParticleSpace},
};

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins,
            camera::CameraPlugin,
            particle::ParticlePlugin,
            space::SpacePlugin,
            emitter::EmitterPlugin,
        ))
        .add_systems(Startup, spawn_scene)
        .run();
}

fn spawn_scene(mut commands: Commands) {
    let space = commands
        .spawn((
            Name::new("Space"),
            ParticleSpace {
                center_pull: Some(CenterPull {
                    strength: 300_000.0,
                    max_accel: 100.0,
                }),
            },
        ))
        .id();

    commands.spawn((
        Name::new("Emitter"),
        ChildOf(space),
        ParticleEmitter {
            burst_count: 2000,
            spawn_half_extents: Vec2::splat(200.0),
            speed: 5.0..50.0,
            radius: 1.0..3.0,
            mass_per_radius: 1.0,
            red: 6.0..9.0,
            green: 0.0..0.0,
            blue: 6.0..9.0,
            alpha: 0.2,
        },
    ));
}
