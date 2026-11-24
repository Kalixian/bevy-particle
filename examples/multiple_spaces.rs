//! Three independent spaces: self-gravitating on the left, plain on the right, and a fountain
//! drawn behind both. `cargo run --example multiple_spaces`
//!
//! `G` toggles the left space's gravity, `M` switches its gravity method, and `Space` removes
//! or restores the right space.

use bevy::prelude::*;
use bevy_particle::{
    Gravity, GravityMethod, Modifier, Modifiers, ParticleEmitter, ParticlePlugin, ParticleSpace,
};

const GRAVITY: Gravity =
    Gravity { strength: 200.0, softening: 8.0, method: GravityMethod::BarnesHut, theta: 0.5 };

#[derive(Component)]
struct Left;
#[derive(Component)]
struct Right;

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, ParticlePlugin))
        .add_systems(Startup, setup)
        .add_systems(Update, controls)
        .run();
}

fn cloud(red: f32, blue: f32) -> ParticleEmitter {
    ParticleEmitter {
        burst_count: 8000,
        rate: 0.0,
        lifetime: None,
        spawn_half_extents: Vec2::splat(120.0),
        speed: (0.0..10.0).into(),
        red: (red..red).into(),
        blue: (blue..blue).into(),
        gravity: true,
        ..default()
    }
}

fn spawn_right(commands: &mut Commands) {
    // The emitter is gravity-enabled too, but this space has no gravity.
    commands.spawn((
        Right,
        ParticleSpace { capacity: 8000 },
        Transform::from_xyz(300.0, 0.0, 1.0),
        children![cloud(0.2, 1.0)],
    ));
}

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
    commands.spawn((
        Left,
        ParticleSpace { capacity: 8000 },
        GRAVITY,
        Transform::from_xyz(-300.0, 0.0, 1.0),
        children![cloud(1.0, 0.2)],
    ));
    spawn_right(&mut commands);
    commands.spawn((
        ParticleSpace { capacity: 20_000 },
        Modifiers(vec![Modifier::Acceleration(Vec2::new(0.0, -150.0).into())]),
        children![(
            Transform::from_xyz(0.0, -300.0, 0.0),
            ParticleEmitter {
                rate: 4000.0,
                speed: (300.0..400.0).into(),
                angle: (1.2..1.9).into(),
                ..default()
            },
        )],
    ));
}

fn controls(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut left: Single<(Entity, Option<&mut Gravity>), With<Left>>,
    right: Option<Single<Entity, With<Right>>>,
) {
    let (entity, gravity) = &mut *left;
    if keys.just_pressed(KeyCode::KeyG) {
        match gravity {
            Some(_) => commands.entity(*entity).remove::<Gravity>(),
            None => commands.entity(*entity).insert(GRAVITY),
        };
    }
    if let Some(gravity) = gravity && keys.just_pressed(KeyCode::KeyM) {
        gravity.method = match gravity.method {
            GravityMethod::BruteForce => GravityMethod::BarnesHut,
            GravityMethod::BarnesHut => GravityMethod::BruteForce,
        };
    }
    if keys.just_pressed(KeyCode::Space) {
        match right {
            Some(right) => commands.entity(*right).despawn(),
            None => spawn_right(&mut commands),
        }
    }
}
