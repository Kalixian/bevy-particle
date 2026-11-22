use std::ops::Range;

use bevy::{
    prelude::*,
    render::{extract_resource::ExtractResource, render_resource::ShaderType},
};

use crate::{
    gpu::GpuSimReady,
    modifier::{ModifierSystems, ModifierUpload, Modifiers},
};

/// The middle layer: emitters decide where, how often, and with what initial values
/// particles spawn, and how long they live.
pub struct EmitterPlugin;

impl Plugin for EmitterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EmitterRegistry>()
            .init_resource::<EmitterUpload>()
            .add_systems(
                PostUpdate,
                (register_emitters, prepare_emitter_upload)
                    .chain()
                    .in_set(EmitterSystems)
                    .after(ModifierSystems::Clear)
                    // Emitter positions come from GlobalTransform, so wait for propagation.
                    .after(TransformSystems::Propagate),
            );
    }
}

/// Builds this frame's [`EmitterUpload`]. Anything reading it should run after this set.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmitterSystems;

/// Spawn settings for a set of particles. Spawn it as a child of a `ParticleSpace`.
///
/// Spawning happens on the GPU: each frame the CPU only sends how many particles each
/// emitter wants; the GPU picks free slots and rolls the random initial values.
/// If the space is full, spawns that don't fit are dropped.
#[derive(Component, Debug, Clone)]
#[require(Transform, EmitterState)]
pub struct ParticleEmitter {
    /// Particles spawned once, on the first frame after the emitter is added.
    pub burst_count: u32,
    /// Continuous spawn rate, in particles per second.
    pub rate: f32,
    /// Lifespan in seconds, sampled per particle. `None` = particles never die.
    pub lifetime: Option<Range<f32>>,
    /// Particles spawn uniformly inside this rectangle, centered on the emitter.
    pub spawn_half_extents: Vec2,
    /// Initial speed.
    pub speed: Range<f32>,
    /// Initial direction in radians (0 = +X, counter-clockwise). `0.0..TAU` = any direction.
    pub angle: Range<f32>,
    pub radius: Range<f32>,
    /// Mass = radius × this value.
    pub mass_per_radius: f32,
    /// Per-channel sRGB ranges (values above 1.0 are HDR and drive bloom).
    pub red: Range<f32>,
    pub green: Range<f32>,
    pub blue: Range<f32>,
    pub alpha: f32,
}

impl Default for ParticleEmitter {
    fn default() -> Self {
        Self {
            burst_count: 0,
            rate: 100.0,
            lifetime: Some(2.0..4.0),
            spawn_half_extents: Vec2::ZERO,
            speed: 10.0..50.0,
            angle: 0.0..std::f32::consts::TAU,
            radius: 1.0..2.0,
            mass_per_radius: 1.0,
            red: 1.0..1.0,
            green: 1.0..1.0,
            blue: 1.0..1.0,
            alpha: 1.0,
        }
    }
}

/// CPU-side spawn bookkeeping for one emitter.
#[derive(Component, Debug, Default)]
pub struct EmitterState {
    /// Fractional particles carried over between frames, so low rates still spawn.
    accumulator: f32,
    burst_done: bool,
}

/// Maps emitter entities to their index in the GPU emitter array.
///
/// Particles store that index, so it must stay stable while the emitter exists. Slots of
/// despawned emitters stay inactive (their particles die) and aren't reused yet.
#[derive(Resource, Default)]
pub struct EmitterRegistry {
    slots: Vec<Option<Entity>>,
}

/// One emitter as seen by the GPU. Must match `struct Emitter` in `particle_sim.wgsl`.
#[derive(ShaderType, Clone, Copy, Debug, Default)]
pub struct GpuEmitter {
    pub position: Vec2,
    pub spawn_half_extents: Vec2,
    pub speed: Vec2,
    pub angle: Vec2,
    pub radius: Vec2,
    /// (min, max); min < 0 means immortal.
    pub lifetime: Vec2,
    /// sRGB, converted to linear on the GPU after sampling.
    pub color_min: Vec4,
    pub color_max: Vec4,
    pub mass_per_radius: f32,
    /// This emitter's spawn requests are spawn-pass threads
    /// `spawn_offset .. spawn_offset + spawn_count`.
    pub spawn_offset: u32,
    pub spawn_count: u32,
    /// 0 for despawned emitters; their particles are killed.
    pub enabled: u32,
    /// This emitter's slice of the modifier array.
    pub modifier_offset: u32,
    pub modifier_count: u32,
}

/// This frame's emitter array and spawn requests, sent to the render world.
#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub struct EmitterUpload {
    /// Indexed by registry slot. Never empty (padded with an inactive emitter),
    /// because a zero-sized storage buffer can't be bound.
    pub emitters: Vec<GpuEmitter>,
    /// Sum of all `spawn_count`s: the number of spawn-pass threads.
    pub spawn_total: u32,
}

fn register_emitters(
    mut registry: ResMut<EmitterRegistry>,
    added: Query<Entity, Added<ParticleEmitter>>,
    mut removed: RemovedComponents<ParticleEmitter>,
) {
    for entity in removed.read() {
        if let Some(slot) = registry.slots.iter_mut().find(|s| **s == Some(entity)) {
            *slot = None;
        }
    }
    for entity in &added {
        registry.slots.push(Some(entity));
    }
}

fn prepare_emitter_upload(
    time: Res<Time>,
    registry: Res<EmitterRegistry>,
    sim_ready: Res<GpuSimReady>,
    mut emitters: Query<(
        &ParticleEmitter,
        &mut EmitterState,
        &GlobalTransform,
        Option<&Modifiers>,
    )>,
    mut upload: ResMut<EmitterUpload>,
    mut modifier_upload: ResMut<ModifierUpload>,
) {
    // Hold spawns (bursts included) until the GPU simulation is actually running,
    // otherwise they'd be requested while the compute pipelines are still compiling and lost.
    let dt = if sim_ready.get() { time.delta_secs() } else { 0.0 };
    let can_burst = sim_ready.get();
    let mut spawn_total = 0u32;

    upload.emitters.clear();
    for slot in &registry.slots {
        let Some((emitter, mut state, transform, modifiers)) =
            slot.and_then(|e| emitters.get_mut(e).ok())
        else {
            upload.emitters.push(GpuEmitter::default());
            continue;
        };

        state.accumulator += emitter.rate.max(0.0) * dt;
        let mut spawn_count = state.accumulator as u32;
        state.accumulator -= spawn_count as f32;
        if can_burst && !state.burst_done {
            state.burst_done = true;
            spawn_count += emitter.burst_count;
        }

        let position = transform.translation().truncate();
        let (modifier_offset, modifier_count) = modifier_upload.push(modifiers, position);

        upload.emitters.push(GpuEmitter {
            position,
            spawn_half_extents: emitter.spawn_half_extents,
            speed: range_vec(&emitter.speed),
            angle: range_vec(&emitter.angle),
            radius: range_vec(&emitter.radius),
            lifetime: emitter.lifetime.as_ref().map_or(Vec2::splat(-1.0), range_vec),
            color_min: Vec4::new(emitter.red.start, emitter.green.start, emitter.blue.start, emitter.alpha),
            color_max: Vec4::new(emitter.red.end, emitter.green.end, emitter.blue.end, emitter.alpha),
            mass_per_radius: emitter.mass_per_radius,
            spawn_offset: spawn_total,
            spawn_count,
            enabled: 1,
            modifier_offset,
            modifier_count,
        });
        spawn_total += spawn_count;
    }

    if upload.emitters.is_empty() {
        upload.emitters.push(GpuEmitter::default());
    }
    upload.spawn_total = spawn_total;
}

fn range_vec(range: &Range<f32>) -> Vec2 {
    Vec2::new(range.start, range.end)
}
