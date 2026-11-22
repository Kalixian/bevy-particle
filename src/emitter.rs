use std::ops::Range;

use bevy::{prelude::*, sprite_render::AlphaMode2d};

use crate::particle::{EmittedBy, Mass, Particle, ParticleMesh, ParticleSystems, Velocity};

/// The middle layer: spawns particles and owns them.
pub struct EmitterPlugin;

impl Plugin for EmitterPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, spawn_bursts.in_set(ParticleSystems::Spawn));
    }
}

/// Spawn settings for a set of particles. Spawn it as a child of a `ParticleSpace`.
///
/// Phase 0 spawns `burst_count` particles once, when the emitter is added.
/// Spawn rate and lifespan come in phase 2.
#[derive(Component, Debug, Clone)]
#[require(Transform)]
pub struct ParticleEmitter {
    pub burst_count: u32,
    /// Particles spawn uniformly inside this rectangle, centered on the emitter.
    pub spawn_half_extents: Vec2,
    /// Initial speed; the direction is random.
    pub speed: Range<f32>,
    pub radius: Range<f32>,
    /// Mass = radius × this value.
    pub mass_per_radius: f32,
    /// Per-channel sRGB ranges (values above 1.0 are HDR and drive bloom).
    pub red: Range<f32>,
    pub green: Range<f32>,
    pub blue: Range<f32>,
    pub alpha: f32,
}

fn spawn_bursts(
    mut commands: Commands,
    mut materials: ResMut<Assets<ColorMaterial>>,
    particle_mesh: Res<ParticleMesh>,
    emitters: Query<(Entity, &ParticleEmitter, &Transform), Added<ParticleEmitter>>,
) {
    for (emitter_entity, emitter, emitter_transform) in &emitters {
        let origin = emitter_transform.translation.truncate();

        for _ in 0..emitter.burst_count {
            let radius = sample(&emitter.radius);
            let position = origin
                + Vec2::new(
                    sample(&(-emitter.spawn_half_extents.x..emitter.spawn_half_extents.x)),
                    sample(&(-emitter.spawn_half_extents.y..emitter.spawn_half_extents.y)),
                );
            let angle = sample(&(0.0..std::f32::consts::TAU));
            let speed = sample(&emitter.speed);
            let color = Color::srgba(
                sample(&emitter.red),
                sample(&emitter.green),
                sample(&emitter.blue),
                emitter.alpha,
            );

            commands.spawn((
                Particle,
                EmittedBy(emitter_entity),
                Velocity(Vec2::from_angle(angle) * speed),
                Mass(radius * emitter.mass_per_radius),
                Transform::from_translation(position.extend(0.0))
                    .with_scale(Vec3::splat(radius)),
                Mesh2d(particle_mesh.0.clone()),
                MeshMaterial2d(materials.add(ColorMaterial {
                    color,
                    alpha_mode: AlphaMode2d::Blend,
                    ..default()
                })),
            ));
        }
    }
}

/// Uniform sample from `range`; returns `start` for an empty range instead of panicking.
fn sample(range: &Range<f32>) -> f32 {
    range.start + (range.end - range.start) * rand::random::<f32>()
}
