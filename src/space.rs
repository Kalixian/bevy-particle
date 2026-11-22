use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::{extract_resource::ExtractResource, render_resource::ShaderType, storage::ShaderBuffer},
};

use crate::{
    emitter::ParticleEmitter,
    particle::{GpuParticle, ParticleBuffer},
};

/// The outer layer: settings and forces that apply to every particle in the space.
pub struct SpacePlugin;

impl Plugin for SpacePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SimParams>()
            .add_systems(PostStartup, build_particle_buffer)
            .add_systems(Update, update_sim_params);
    }
}

/// A particle space. Emitters belong to a space as its children (`ChildOf`).
///
/// Phase 1 supports one space; the GPU buffers are global resources for now.
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

/// Per-frame simulation uniforms. Must match `struct SimParams` in `particle_integrate.wgsl`.
#[derive(Resource, Clone, Copy, Default, ExtractResource, ShaderType)]
#[extract_app(bevy::render::RenderApp)]
pub struct SimParams {
    pub center: Vec2,
    pub pull_strength: f32,
    pub pull_max_accel: f32,
    pub dt: f32,
    pub count: u32,
}

/// Generates every emitter's burst on the CPU and uploads it as one GPU buffer.
fn build_particle_buffer(
    mut commands: Commands,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    spaces: Query<(Entity, &Transform, &Children), With<ParticleSpace>>,
    emitters: Query<(&ParticleEmitter, &Transform)>,
) {
    let mut spaces = spaces.iter();
    let Some((space, space_transform, children)) = spaces.next() else {
        warn!("No ParticleSpace found; nothing to simulate");
        return;
    };
    if spaces.next().is_some() {
        warn!("Only one ParticleSpace is supported in phase 1; using {space}");
    }

    let particles: Vec<GpuParticle> = emitters
        .iter_many(children)
        // Children that aren't emitters come back as Err; skip them.
        .flatten()
        // GlobalTransform isn't propagated yet during PostStartup, so compose by hand.
        .flat_map(|(emitter, transform)| {
            let origin = space_transform.mul_transform(*transform).translation.truncate();
            emitter.burst(origin)
        })
        .collect();
    let count = particles.len() as u32;
    info!("Uploading {count} particles to the GPU");

    // RENDER_WORLD only: the CPU copy is dropped after upload, and the GPU owns the state.
    let buffer = buffers.add(ShaderBuffer::new(particles, RenderAssetUsages::RENDER_WORLD));
    commands.insert_resource(ParticleBuffer { buffer, count });
}

fn update_sim_params(
    time: Res<Time>,
    particle_buffer: Option<Res<ParticleBuffer>>,
    space: Query<(&ParticleSpace, &GlobalTransform)>,
    mut params: ResMut<SimParams>,
) {
    let Some((space, transform)) = space.iter().next() else {
        return;
    };
    let pull = space.center_pull.unwrap_or(CenterPull {
        strength: 0.0,
        max_accel: 0.0,
    });
    *params = SimParams {
        center: transform.translation().truncate(),
        pull_strength: pull.strength,
        pull_max_accel: pull.max_accel,
        dt: time.delta_secs(),
        count: particle_buffer.map_or(0, |b| b.count),
    };
}
