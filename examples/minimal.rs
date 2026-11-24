//! A fountain built in code, without asset files: `cargo run --example minimal`.

use bevy::prelude::*;
use bevy_particle::{
    Blend, Modifier, Modifiers, ParticleEmitter, ParticlePlugin, ParticleSpace, Track,
};

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, ParticlePlugin))
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
    commands.spawn((
        ParticleSpace { capacity: 50_000 },
        Modifiers(vec![Modifier::Acceleration(Vec2::new(0.0, -200.0).into())]),
        children![(
            Transform::from_xyz(0.0, -200.0, 0.0),
            ParticleEmitter {
                rate: 5000.0,
                speed: (250.0..400.0).into(),
                angle: (1.3..1.8).into(),
                ..default()
            },
            Modifiers(vec![Modifier::Alpha(
                Blend::Set,
                Track::over_life([(0.0, 1.0), (1.0, 0.0)]).into(),
            )]),
        )],
    ));
}
