use bevy::{
    ecs::entity::EntityHashMap,
    prelude::*,
    render::{extract_resource::ExtractResource, render_resource::ShaderType},
};
use serde::{Deserialize, Serialize};

use crate::{
    gpu::GpuSimReady,
    modifier::{ModifierSystems, ModifierUpload, Modifiers, TargetAttractor},
    particle::ParticleBuffers,
    space::ParticleSpace,
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

/// Spawn settings for a set of particles. It spawns into the nearest [`ParticleSpace`] among
/// its ancestors, so make it a child (or grandchild) of one; without one it does nothing.
///
/// Moving an emitter to another space only affects new particles; existing ones stay behind.
///
/// Spawning happens on the GPU: each frame the CPU only sends how many particles each
/// emitter wants; the GPU picks free slots and rolls the random initial values.
/// If the space is full, spawns that don't fit are dropped.
#[derive(Component, Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
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
    /// Alpha range (opacity).
    pub alpha: Spread,
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
            alpha: (1.0..1.0).into(),
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
    /// Seconds since the emitter started, for `Track::over_time`.
    age: f32,
    warned_no_space: bool,
}

/// Emitter references have 16 bits for the slot.
const MAX_EMITTERS: usize = 1 << 16;

/// Maps emitter entities to their slot (index) in the GPU emitter array.
///
/// Particles store their emitter's slot and the slot's generation. A slot keeps its place
/// while its emitter exists and is reused afterwards; its generation goes up each time it's
/// released, so the GPU can tell (and kill) particles left over from an earlier emitter.
/// Slots are never removed, since that would forget their generations.
#[derive(Resource, Default)]
pub struct EmitterRegistry {
    slots: Vec<EmitterSlot>,
}

#[derive(Debug, Default)]
struct EmitterSlot {
    entity: Option<Entity>,
    /// 16 bits to fit in an emitter reference. Wrapping is fine: every simulated frame kills
    /// the particles of older generations.
    generation: u16,
}

impl EmitterRegistry {
    /// Gives `entity` a slot. `false` if every slot is taken.
    fn register(&mut self, entity: Entity) -> bool {
        if let Some(slot) = self.slots.iter_mut().find(|s| s.entity.is_none()) {
            slot.entity = Some(entity);
        } else if self.slots.len() < MAX_EMITTERS {
            self.slots.push(EmitterSlot { entity: Some(entity), generation: 0 });
        } else {
            return false;
        }
        true
    }

    fn release(&mut self, entity: Entity) {
        if let Some(slot) = self.slots.iter_mut().find(|s| s.entity == Some(entity)) {
            slot.entity = None;
            slot.generation = slot.generation.wrapping_add(1);
        }
    }
}

/// One emitter as seen by the GPU. Must match `struct Emitter` in `particle_sim.wesl`.
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
    pub alpha: Vec2,
    pub mass_per_radius: f32,
    /// Entry i of the spawn table (see [`SpawnTable`]) is kept in array element i, to save a
    /// binding; it isn't about this emitter.
    pub table_first: u32,
    pub table_emitter: u32,
    /// 0 for despawned emitters; their particles are killed.
    pub enabled: u32,
    /// The slot's generation; particles of any other generation are killed.
    pub generation: u32,
    /// This emitter's slice of the modifier array; see `ModifierUpload::push`.
    pub modifiers: UVec4,
    /// 1 if this emitter's particles take part in gravity.
    pub gravity: u32,
    pub speed_curve: u32,
    pub angle_curve: u32,
    pub radius_curve: u32,
    pub lifetime_curve: u32,
    pub red_curve: u32,
    pub green_curve: u32,
    pub blue_curve: u32,
    pub alpha_curve: u32,
}

/// This frame's emitter array (shared by all spaces), sent to the render world.
#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub struct EmitterUpload {
    /// Indexed by registry slot. Never empty (padded with an inactive emitter),
    /// because a zero-sized storage buffer can't be bound.
    pub emitters: Vec<GpuEmitter>,
    /// Lookup tables for the emitters' non-uniform `Spread`s.
    curves: Vec<GpuCurve>,
    pub spawns: SpawnTable,
}

/// This frame's spawn requests. They're numbered so that each space's are contiguous.
#[derive(Clone, Default)]
pub struct SpawnTable {
    /// Spaces without requests are left out.
    pub ranges: EntityHashMap<SpawnRange>,
    /// (first request, emitter slot) for each spawning emitter, in request order.
    entries: Vec<(u32, u32)>,
}

/// A space's spawn requests: `first .. first + count`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpawnRange {
    pub first: u32,
    pub count: u32,
}

impl SpawnTable {
    /// `emitters` holds the space each emitter spawns into, and how many particles.
    fn new(emitters: &[(Option<Entity>, u32)]) -> Self {
        let mut by_space = EntityHashMap::<Vec<(u32, u32)>>::default();
        for (slot, (space, count)) in emitters.iter().enumerate() {
            if let Some(space) = space && *count > 0 {
                by_space.entry(*space).or_default().push((slot as u32, *count));
            }
        }
        let mut table = Self::default();
        let mut next = 0u32;
        for (space, emitters) in by_space {
            let first = next;
            for (slot, count) in emitters {
                table.entries.push((next, slot));
                next = next.saturating_add(count);
            }
            table.ranges.insert(space, SpawnRange { first, count: next - first });
        }
        table
    }

    pub fn len(&self) -> u32 {
        self.entries.len() as u32
    }
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
        registry.release(entity);
    }
    for entity in &added {
        if !registry.register(entity) {
            warn!("Only {MAX_EMITTERS} emitters are supported; ignoring {entity}");
        }
    }
}

fn prepare_emitter_upload(
    time: Res<Time>,
    registry: Res<EmitterRegistry>,
    sim_ready: Res<GpuSimReady>,
    spaces: Query<&ParticleBuffers, With<ParticleSpace>>,
    is_space: Query<(), With<ParticleSpace>>,
    parents: Query<&ChildOf>,
    mut emitters: Query<(
        &ParticleEmitter,
        &mut EmitterState,
        &GlobalTransform,
        Option<&Modifiers>,
        Option<&TargetAttractor>,
    )>,
    mut upload: ResMut<EmitterUpload>,
    mut modifier_upload: ResMut<ModifierUpload>,
) {
    // The space and spawn count of each slot.
    let mut spawns = Vec::with_capacity(registry.slots.len());

    upload.emitters.clear();
    upload.curves.clear();
    for slot in &registry.slots {
        let Some((entity, (emitter, mut state, transform, modifiers, attractor))) =
            slot.entity.and_then(|e| Some((e, emitters.get_mut(e).ok()?)))
        else {
            upload.emitters.push(GpuEmitter::default());
            spawns.push((None, 0));
            continue;
        };

        let space = parents.iter_ancestors(entity).find(|e| is_space.contains(*e));
        if space.is_none() && !state.warned_no_space {
            warn!("Emitter {entity} has no ParticleSpace among its ancestors");
            state.warned_no_space = true;
        }
        // Hold spawns (bursts included) until the GPU is actually simulating the space. Before
        // that (pipelines compiling, buffers not created yet) they'd be lost.
        let buffers = space
            .and_then(|space| spaces.get(space).ok())
            .filter(|buffers| sim_ready.is_simulating(buffers.id()));
        let can_burst = buffers.is_some();
        let dt = if can_burst { time.delta_secs() } else { 0.0 };

        state.accumulator += emitter.rate.max(0.0) * dt;
        let mut spawn_count = state.accumulator as u32;
        state.accumulator -= spawn_count as f32;
        if can_burst && !state.burst_done {
            state.burst_done = true;
            spawn_count = spawn_count.saturating_add(emitter.burst_count);
        }
        // More could never fit.
        spawn_count = spawn_count.min(buffers.map_or(0, |buffers| buffers.capacity));
        spawns.push((space, spawn_count));

        let position = transform.translation().truncate();
        let modifiers = modifier_upload.push(modifiers, attractor, position, state.age);
        state.age += dt;

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
        let (alpha, alpha_curve) = emitter.alpha.to_gpu(curves);

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
            alpha,
            mass_per_radius: emitter.mass_per_radius,
            // The spawn table is filled in below.
            table_first: 0,
            table_emitter: 0,
            enabled: 1,
            generation: slot.generation.into(),
            modifiers,
            gravity: emitter.gravity as u32,
            speed_curve,
            angle_curve,
            radius_curve,
            lifetime_curve,
            red_curve,
            green_curve,
            blue_curve,
            alpha_curve,
        });
    }

    let upload = &mut *upload;
    upload.spawns = SpawnTable::new(&spawns);
    for (emitter, entry) in upload.emitters.iter_mut().zip(&upload.spawns.entries) {
        (emitter.table_first, emitter.table_emitter) = *entry;
    }
    if upload.emitters.is_empty() {
        upload.emitters.push(GpuEmitter::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reused_slots_get_a_new_generation() {
        let mut world = World::new();
        let [a, b, c] = std::array::from_fn(|_| world.spawn_empty().id());
        let mut registry = EmitterRegistry::default();
        assert!(registry.register(a) && registry.register(b));

        registry.release(a);
        assert!(registry.register(c));
        let slots: Vec<_> = registry.slots.iter().map(|s| (s.entity, s.generation)).collect();
        assert_eq!(slots, [(Some(c), 1), (Some(b), 0)]);

        // Generations survive every emitter being gone.
        registry.release(b);
        registry.release(c);
        assert!(registry.register(a));
        let first = &registry.slots[0];
        assert_eq!((first.entity, first.generation), (Some(a), 2));
    }

    #[test]
    fn spawn_requests_are_contiguous_per_space() {
        let mut world = World::new();
        let [a, b, idle] = std::array::from_fn(|_| world.spawn_empty().id());
        let table = SpawnTable::new(&[
            (Some(a), 3),
            (Some(b), 5),
            (Some(a), 2),
            (Some(idle), 0),
            (None, 9),
            (Some(b), 7),
        ]);
        let (range_a, range_b) = (table.ranges[&a], table.ranges[&b]);
        assert_eq!((range_a.count, range_b.count, table.ranges.len()), (5, 12, 2));
        // One range follows the other.
        assert!(
            (range_a.first, range_b.first) == (0, 5) || (range_b.first, range_a.first) == (0, 12)
        );
        let first = |slot| table.entries.iter().find(|(_, s)| *s == slot).unwrap().0;
        assert_eq!((first(0), first(2)), (range_a.first, range_a.first + 3));
        assert_eq!((first(1), first(5)), (range_b.first, range_b.first + 5));
        // Sorted, for the GPU's binary search.
        assert!(table.entries.is_sorted() && table.len() == 4);
    }

    #[test]
    fn registering_fails_when_every_slot_is_taken() {
        let mut registry = EmitterRegistry::default();
        registry.slots.resize_with(MAX_EMITTERS, || EmitterSlot {
            entity: Some(Entity::PLACEHOLDER),
            generation: 0,
        });
        assert!(!registry.register(World::new().spawn_empty().id()));
    }
}
