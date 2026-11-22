use bevy::{
    prelude::*,
    render::{extract_resource::ExtractResource, render_resource::ShaderType},
};

use crate::{
    gpu::GpuSimReady,
    modifier::{ModifierSystems, ModifierUpload, Modifiers, MouseAttractor},
    spread::{GpuCurve, Spread, NO_CURVE},
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
    ///
    /// This and the other `Spread` fields are random ranges with a distribution shape:
    /// `(1.0..2.0).into()` is uniform; see [`Spread`] and `Shape` for skewed ones.
    pub lifetime: Option<Spread>,
    /// Particles spawn uniformly inside this rectangle, centered on the emitter.
    pub spawn_half_extents: Vec2,
    /// Initial speed.
    pub speed: Spread,
    /// Initial direction in radians (0 = +X, counter-clockwise). `0.0..TAU` = any direction.
    pub angle: Spread,
    pub radius: Spread,
    /// Mass = radius × this value.
    pub mass_per_radius: f32,
    /// Per-channel sRGB ranges (values above 1.0 are HDR and drive bloom).
    pub red: Spread,
    pub green: Spread,
    pub blue: Spread,
    pub alpha: f32,
    /// Whether this emitter's particles take part in the space's `Gravity`
    /// (attract each other and feel the pull). Brute-force cost grows with the square of
    /// all participating particles, so turn it on only for modest counts for now.
    pub gravity: bool,
}

impl Default for ParticleEmitter {
    fn default() -> Self {
        Self {
            burst_count: 0,
            rate: 100.0,
            lifetime: Some((2.0..4.0).into()),
            spawn_half_extents: Vec2::ZERO,
            speed: (10.0..50.0).into(),
            angle: (0.0..std::f32::consts::TAU).into(),
            radius: (1.0..2.0).into(),
            mass_per_radius: 1.0,
            red: (1.0..1.0).into(),
            green: (1.0..1.0).into(),
            blue: (1.0..1.0).into(),
            alpha: 1.0,
            gravity: false,
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
/// despawned emitters stay inactive (their particles die) and aren't reused, except that
/// the whole list starts over once every emitter is gone.
#[derive(Resource, Default)]
pub struct EmitterRegistry {
    slots: Vec<Option<Entity>>,
}

/// One emitter as seen by the GPU. Must match `struct Emitter` in `particle_sim.wgsl`.
///
/// Each `Vec2` range is (start, end) and has a `*_curve` index into the curve array
/// (`NO_CURVE` for uniform).
#[derive(ShaderType, Clone, Copy, Debug, Default)]
pub struct GpuEmitter {
    pub position: Vec2,
    pub spawn_half_extents: Vec2,
    pub speed: Vec2,
    pub angle: Vec2,
    pub radius: Vec2,
    /// start < 0 means immortal.
    pub lifetime: Vec2,
    /// sRGB channel ranges, converted to linear on the GPU after sampling.
    pub red: Vec2,
    pub green: Vec2,
    pub blue: Vec2,
    pub alpha: f32,
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
    /// 1 if this emitter's particles take part in gravity.
    pub gravity: u32,
    pub speed_curve: u32,
    pub angle_curve: u32,
    pub radius_curve: u32,
    pub lifetime_curve: u32,
    pub red_curve: u32,
    pub green_curve: u32,
    pub blue_curve: u32,
}

/// This frame's emitter array and spawn requests, sent to the render world.
#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub struct EmitterUpload {
    /// Indexed by registry slot. Never empty (padded with an inactive emitter),
    /// because a zero-sized storage buffer can't be bound.
    pub emitters: Vec<GpuEmitter>,
    /// Lookup tables for the emitters' non-uniform `Spread`s.
    curves: Vec<GpuCurve>,
    /// Sum of all `spawn_count`s: the number of spawn-pass threads.
    pub spawn_total: u32,
}

impl EmitterUpload {
    /// The curve array to upload. Never empty, because a zero-sized storage buffer can't be
    /// bound.
    pub fn curves_gpu_data(&self) -> Vec<GpuCurve> {
        if self.curves.is_empty() {
            vec![GpuCurve::default()]
        } else {
            self.curves.clone()
        }
    }
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
    // With every emitter gone (e.g. the space was reset), start numbering from 0 again.
    if registry.slots.iter().all(Option::is_none) {
        registry.slots.clear();
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
        Option<&MouseAttractor>,
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
    upload.curves.clear();
    for slot in &registry.slots {
        let Some((emitter, mut state, transform, modifiers, mouse)) =
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
        let (modifier_offset, modifier_count) = modifier_upload.push(modifiers, mouse, position);

        let curves = &mut upload.curves;
        let (speed, speed_curve) = emitter.speed.to_gpu(curves);
        let (angle, angle_curve) = emitter.angle.to_gpu(curves);
        let (radius, radius_curve) = emitter.radius.to_gpu(curves);
        let (lifetime, lifetime_curve) = match &emitter.lifetime {
            Some(lifetime) => lifetime.to_gpu(curves),
            None => (Vec2::splat(-1.0), NO_CURVE),
        };
        let (red, red_curve) = emitter.red.to_gpu(curves);
        let (green, green_curve) = emitter.green.to_gpu(curves);
        let (blue, blue_curve) = emitter.blue.to_gpu(curves);

        upload.emitters.push(GpuEmitter {
            position,
            spawn_half_extents: emitter.spawn_half_extents,
            speed,
            angle,
            radius,
            lifetime,
            red,
            green,
            blue,
            alpha: emitter.alpha,
            mass_per_radius: emitter.mass_per_radius,
            spawn_offset: spawn_total,
            spawn_count,
            enabled: 1,
            modifier_offset,
            modifier_count,
            gravity: emitter.gravity as u32,
            speed_curve,
            angle_curve,
            radius_curve,
            lifetime_curve,
            red_curve,
            green_curve,
            blue_curve,
        });
        spawn_total += spawn_count;
    }

    if upload.emitters.is_empty() {
        upload.emitters.push(GpuEmitter::default());
    }
    upload.spawn_total = spawn_total;
}
