use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::{extract_resource::ExtractResource, render_resource::ShaderType, storage::ShaderBuffer},
};
use serde::{Deserialize, Serialize};

use crate::{
    emitter::{EmitterSystems, EmitterUpload},
    gpu::GpuSimReady,
    gravity::{Gravity, MAX_GRAVITY_BODIES},
    modifier::{ModifierSystems, ModifierUpload, Modifiers, TargetAttractor},
    particle::{GpuParticle, ParticleBuffers},
};

/// The outer layer: settings and forces that apply to every particle in the space.
/// Space-wide behavior comes from a [`Modifiers`] component on the space entity, and
/// particle-to-particle attraction from a [`Gravity`] component.
pub struct SpacePlugin;

impl Plugin for SpacePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SimParams>().add_systems(
            PostUpdate,
            (
                // Before the emitters, so a replaced space holds this frame's spawns.
                sync_space_buffers.before(EmitterSystems),
                update_sim_params
                    .after(sync_space_buffers)
                    .after(EmitterSystems)
                    .after(ModifierSystems::Clear),
            )
                .after(TransformSystems::Propagate),
        );
    }
}

/// A particle space. Emitters belong to a space as their parent (`ChildOf`).
///
/// Only one space is supported for now: the GPU buffers are global resources.
/// Despawning the space (which also despawns its emitters) frees its particles; spawning a
/// new one starts from an empty buffer.
#[derive(Component, Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[require(Transform, SpaceState)]
pub struct ParticleSpace {
    /// Maximum number of live particles. Fixed once the space is created.
    pub capacity: u32,
}

/// CPU-side bookkeeping for a space.
#[derive(Component, Debug, Default)]
pub struct SpaceState {
    /// Seconds since the space started, for `Track::over_time`.
    age: f32,
}

/// Per-frame simulation uniforms. Must match `struct SimParams` in `types.wesl`.
#[derive(Resource, Clone, Copy, Debug, Default, ExtractResource, ShaderType)]
#[extract_app(bevy::render::RenderApp)]
pub struct SimParams {
    pub dt: f32,
    /// Number of particle slots (integrate-pass threads).
    pub capacity: u32,
    /// Number of spawn requests this frame (spawn-pass threads).
    pub spawn_total: u32,
    pub emitter_count: u32,
    /// Changes every frame; seeds the GPU random numbers.
    pub frame: u32,
    /// The space's slice of the modifier array.
    pub modifier_offset: u32,
    pub modifier_count: u32,
    /// 1 when the space has [`Gravity`].
    pub gravity_enabled: u32,
    pub gravity_strength: f32,
    pub gravity_softening_sq: f32,
    /// 0 = brute force, 1 = Barnes-Hut. See [`GravityMethod::as_gpu`].
    pub gravity_method: u32,
    /// Most particles that can take part in gravity (sizes the GPU work buffers).
    pub gravity_max_bodies: u32,
    /// Barnes-Hut opening angle θ, squared.
    pub gravity_theta_sq: f32,
}

/// Drops the GPU buffers of a despawned space and creates buffers for a newly added one.
fn sync_space_buffers(
    mut commands: Commands,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    added: Query<(Entity, &ParticleSpace), Added<ParticleSpace>>,
    mut removed: RemovedComponents<ParticleSpace>,
    existing: Option<Res<ParticleBuffers>>,
) {
    let owner_removed = existing
        .as_ref()
        .is_some_and(|b| removed.read().any(|entity| entity == b.space));
    if owner_removed {
        commands.remove_resource::<ParticleBuffers>();
    }
    let mut has_buffers = existing.is_some() && !owner_removed;

    for (entity, space) in &added {
        if has_buffers {
            warn!("Only one ParticleSpace is supported; ignoring {entity}");
            continue;
        }
        has_buffers = true;
        let capacity = space.capacity;
        info!("Allocating {capacity} particle slots");

        // Zero-initialized by wgpu, which means every slot starts dead.
        let particles = ShaderBuffer::with_size(
            capacity as u64 * size_of::<GpuParticle>() as u64,
            RenderAssetUsages::RENDER_WORLD,
        );
        // [count, then indices capacity-1 .. 0] so the first pops hand out low indices.
        let free_list: Vec<u32> = std::iter::once(capacity).chain((0..capacity).rev()).collect();
        let free_list = ShaderBuffer::new(free_list, RenderAssetUsages::RENDER_WORLD);
        // `capacity` × vec2<f32>: gravity acceleration per slot, read by `integrate`.
        let accel =
            ShaderBuffer::with_size(capacity as u64 * 8, RenderAssetUsages::RENDER_WORLD);

        commands.insert_resource(ParticleBuffers {
            space: entity,
            particles: buffers.add(particles),
            free_list: buffers.add(free_list),
            accel: buffers.add(accel),
            capacity,
        });
    }
}

fn update_sim_params(
    time: Res<Time>,
    particle_buffers: Option<Res<ParticleBuffers>>,
    emitters: Res<EmitterUpload>,
    mut modifier_upload: ResMut<ModifierUpload>,
    sim_ready: Res<GpuSimReady>,
    mut space: Query<
        (
            &mut SpaceState,
            &GlobalTransform,
            Option<&Modifiers>,
            Option<&TargetAttractor>,
            Option<&Gravity>,
        ),
        With<ParticleSpace>,
    >,
    mut params: ResMut<SimParams>,
) {
    let (Some((mut state, transform, modifiers, attractor, gravity)), Some(buffers)) =
        (space.iter_mut().next(), particle_buffers)
    else {
        *params = SimParams::default();
        return;
    };
    let (modifier_offset, modifier_count) =
        modifier_upload.push(modifiers, attractor, transform.translation().truncate(), state.age);
    if sim_ready.is_simulating(buffers.space) {
        state.age += time.delta_secs();
    }

    *params = SimParams {
        dt: time.delta_secs(),
        capacity: buffers.capacity,
        spawn_total: emitters.spawn_total,
        emitter_count: emitters.emitters.len() as u32,
        frame: params.frame.wrapping_add(1),
        modifier_offset,
        modifier_count,
        gravity_enabled: gravity.is_some() as u32,
        gravity_strength: gravity.map_or(0.0, |g| g.strength),
        // Must stay above 0: a body's pull on itself is 0 · (0 + ε²)^(-3/2), NaN at ε = 0.
        gravity_softening_sq: gravity.map_or(1.0, |g| g.softening.max(1e-3).powi(2)),
        gravity_method: gravity.map_or(0, |g| g.method.as_gpu()),
        gravity_max_bodies: buffers.capacity.min(MAX_GRAVITY_BODIES),
        gravity_theta_sq: gravity.map_or(0.0, |g| g.theta * g.theta),
    };
}
