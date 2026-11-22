use std::ops::Range;

use bevy::prelude::*;

use crate::particle::GpuParticle;

/// Spawn settings for a set of particles. Spawn it as a child of a `ParticleSpace`.
///
/// Phase 1 spawns `burst_count` particles once, when the space's GPU buffer is built.
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

impl ParticleEmitter {
    /// Generates this emitter's burst around `origin`.
    pub fn burst(&self, origin: Vec2) -> impl Iterator<Item = GpuParticle> + '_ {
        (0..self.burst_count).map(move |_| {
            let radius = sample(&self.radius);
            let position = origin
                + Vec2::new(
                    sample(&(-self.spawn_half_extents.x..self.spawn_half_extents.x)),
                    sample(&(-self.spawn_half_extents.y..self.spawn_half_extents.y)),
                );
            let velocity = Vec2::from_angle(sample(&(0.0..std::f32::consts::TAU))) * sample(&self.speed);
            let color = Color::srgba(
                sample(&self.red),
                sample(&self.green),
                sample(&self.blue),
                self.alpha,
            )
            .to_linear();

            GpuParticle {
                position: position.into(),
                velocity: velocity.into(),
                color: color.to_f32_array(),
                mass: radius * self.mass_per_radius,
                radius,
                _padding: [0.0; 2],
            }
        })
    }
}

/// Uniform sample from `range`; returns `start` for an empty range instead of panicking.
fn sample(range: &Range<f32>) -> f32 {
    range.start + (range.end - range.start) * rand::random::<f32>()
}
