use bevy::prelude::*;

use crate::{
    emitter::ParticleEmitter,
    particle::{EmittedBy, Particle, ParticleSystems, Velocity},
};

/// The outer layer: settings and forces that apply to every particle in the space.
pub struct SpacePlugin;

impl Plugin for SpacePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, apply_space_forces.in_set(ParticleSystems::Forces));
    }
}

/// A particle space. Emitters belong to a space as its children (`ChildOf`).
#[derive(Component, Debug, Clone)]
#[require(Transform)]
pub struct ParticleSpace {
    pub center_pull: Option<CenterPull>,
}

/// Inverse-square pull toward the space's origin.
/// Placeholder until the modifier system (phase 3) and real gravity (phases 4–5).
#[derive(Debug, Clone, Copy)]
pub struct CenterPull {
    /// Acceleration = strength / distance².
    pub strength: f32,
    /// Upper limit on the acceleration, so particles near the center don't blow up.
    pub max_accel: f32,
}

fn apply_space_forces(
    time: Res<Time>,
    spaces: Query<(&ParticleSpace, &GlobalTransform)>,
    emitters: Query<&ChildOf, With<ParticleEmitter>>,
    mut particles: Query<(&Transform, &mut Velocity, &EmittedBy), With<Particle>>,
) {
    let dt = time.delta_secs();
    for (transform, mut velocity, emitted_by) in &mut particles {
        let Ok(child_of) = emitters.get(emitted_by.0) else {
            continue;
        };
        let Ok((space, space_transform)) = spaces.get(child_of.parent()) else {
            continue;
        };

        if let Some(pull) = space.center_pull {
            let offset = space_transform.translation().truncate() - transform.translation.truncate();
            let distance_sq = offset.length_squared();
            if distance_sq > 0.0 {
                let accel = (pull.strength / distance_sq.max(1.0)).clamp(0.0, pull.max_accel);
                **velocity += offset.normalize_or_zero() * accel * dt;
            }
        }
    }
}
