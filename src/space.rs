use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::{RenderLayers, VisibilitySystems},
    prelude::*,
    render::{
        extract_resource::ExtractResource, render_resource::ShaderType, renderer::RenderDevice,
        storage::ShaderBuffer,
    },
};
use serde::{Deserialize, Serialize};

use crate::{
    emitter::{EmitterSystems, EmitterUpload},
    gpu::GpuSimReady,
    gravity::{Gravity, MAX_GRAVITY_BODIES},
    modifier::{ModifierSystems, ModifierUpload, Modifiers, TargetAttractor},
    particle::{GpuParticle, ParticleBuffers},
    render::ParticleRender,
};

/// The outer layer: settings and forces that apply to every particle in the space.
/// Space-wide behavior comes from a [`Modifiers`] component on the space entity, and
/// particle-to-particle attraction from a [`Gravity`] component.
pub struct SpacePlugin;

impl Plugin for SpacePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpaceUploads>().add_systems(
            PostUpdate,
            (
                // Before the emitters, so a replaced space holds this frame's spawns.
                sync_space_buffers.before(EmitterSystems),
                prepare_space_uploads
                    .after(sync_space_buffers)
                    .after(EmitterSystems)
                    .after(ModifierSystems::Clear)
                    .after(VisibilitySystems::VisibilityPropagate),
            )
                .after(TransformSystems::Propagate),
        );
    }
}

/// A particle space: a set of particles that share settings, forces and gravity. Emitters
/// spawn into the nearest space among their ancestors.
///
/// Spaces don't affect each other. Their `Transform`'s z orders them among sprites, and
/// [`ParticleRender`] sets how their particles look. Each has
/// its own GPU buffers, and gravity buffers only while it has [`Gravity`]. Despawning a space
/// (which also despawns its emitters) frees its particles.
#[derive(Component, Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[require(Transform, Visibility, SpaceState)]
pub struct ParticleSpace {
    /// Maximum number of live particles; must be above 0. Lowered, with a warning, if it's more
    /// than the GPU supports: about 2 million on devices with WebGPU's default limits. Fixed once
    /// the space is created.
    pub capacity: u32,
}

/// CPU-side bookkeeping for a space.
#[derive(Component, Debug, Default)]
pub struct SpaceState {
    /// Seconds since the space started, for `Track::over_time`.
    age: f32,
}

/// Per-frame simulation uniforms. Must match `struct SimParams` in `types.wesl`.
#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct SimParams {
    pub dt: f32,
    /// Number of particle slots (integrate-pass threads).
    pub capacity: u32,
    /// The space's spawn requests this frame are numbered from here.
    pub spawn_first: u32,
    /// Number of spawn requests this frame (spawn-pass threads).
    pub spawn_total: u32,
    /// Length of the spawn table, which covers all spaces.
    pub spawn_table_len: u32,
    /// Changes every frame; seeds the GPU random numbers.
    pub frame: u32,
    /// The space's slice of the modifier array; see `ModifierUpload::push`.
    pub modifiers: UVec4,
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

impl SimParams {
    pub fn uses_gravity(&self) -> bool {
        self.gravity_enabled != 0
    }
}

/// Every space with buffers, rebuilt each frame and sent to the render world.
#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub struct SpaceUploads(pub Vec<SpaceUpload>);

#[derive(Clone)]
pub struct SpaceUpload {
    pub entity: Entity,
    pub buffers: ParticleBuffers,
    pub params: SimParams,
    pub render: ParticleRender,
    pub z: f32,
    pub visible: bool,
    pub layers: RenderLayers,
}

/// The most particles a space can have with buffers of at most `buffer_size` bytes and dispatches
/// of at most `workgroups` (of 256 slots).
fn max_capacity(buffer_size: u64, workgroups: u32) -> u32 {
    // The particles are the largest buffer.
    let particles = buffer_size / size_of::<GpuParticle>() as u64;
    particles.min(workgroups as u64 * 256).min(u32::MAX as u64) as u32
}

/// Creates buffers for new spaces and drops those of spaces whose component was removed.
/// (Despawned spaces take theirs with them.)
fn sync_space_buffers(
    mut commands: Commands,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    added: Query<(Entity, &ParticleSpace), Added<ParticleSpace>>,
    mut removed: RemovedComponents<ParticleSpace>,
    device: Option<Res<RenderDevice>>,
) {
    for entity in removed.read() {
        if let Ok(mut entity) = commands.get_entity(entity) {
            entity.try_remove::<ParticleBuffers>();
        }
    }

    for (entity, space) in &added {
        let mut capacity = space.capacity;
        if capacity == 0 {
            warn!("ParticleSpace {entity} has capacity 0; ignoring it");
            continue;
        }
        if let Some(device) = &device {
            let limits = device.limits();
            let max = max_capacity(
                limits.max_storage_buffer_binding_size.min(limits.max_buffer_size),
                limits.max_compute_workgroups_per_dimension,
            );
            if capacity > max {
                warn!("This GPU fits {max} particles per space; ParticleSpace {entity} asked for {capacity}");
                capacity = max;
            }
        }
        info!("Allocating {capacity} particle slots");

        // Zero-initialized by wgpu, which means every slot starts dead.
        let particles = ShaderBuffer::with_size(
            capacity as u64 * size_of::<GpuParticle>() as u64,
            RenderAssetUsages::RENDER_WORLD,
        );
        // [count, then indices capacity-1 .. 0] so the first pops hand out low indices.
        let free_list: Vec<u32> = std::iter::once(capacity).chain((0..capacity).rev()).collect();
        let free_list = ShaderBuffer::new(free_list, RenderAssetUsages::RENDER_WORLD);

        commands.entity(entity).try_insert((
            ParticleBuffers {
                particles: buffers.add(particles),
                free_list: buffers.add(free_list),
                capacity,
            },
            SpaceState::default(),
        ));
    }
}

fn prepare_space_uploads(
    time: Res<Time>,
    emitters: Res<EmitterUpload>,
    mut modifier_upload: ResMut<ModifierUpload>,
    sim_ready: Res<GpuSimReady>,
    mut spaces: Query<
        (
            Entity,
            &ParticleBuffers,
            &mut SpaceState,
            (&GlobalTransform, &InheritedVisibility, Option<&RenderLayers>),
            (Option<&Modifiers>, Option<&TargetAttractor>),
            Option<&Gravity>,
            Option<&ParticleRender>,
        ),
        With<ParticleSpace>,
    >,
    mut frame: Local<u32>,
    mut uploads: ResMut<SpaceUploads>,
) {
    *frame = frame.wrapping_add(1);
    uploads.0.clear();
    for (entity, buffers, mut state, placement, (modifiers, attractor), gravity, render) in
        &mut spaces
    {
        let (transform, visibility, layers) = placement;
        let position = transform.translation();
        let modifiers =
            modifier_upload.push(modifiers, attractor, position.truncate(), state.age);
        if sim_ready.is_simulating(buffers.id()) {
            state.age += time.delta_secs();
        }
        let spawns = emitters.spawns.ranges.get(&entity).copied().unwrap_or_default();

        let params = SimParams {
            dt: time.delta_secs(),
            capacity: buffers.capacity,
            spawn_first: spawns.first,
            // More could never fit.
            spawn_total: spawns.count.min(buffers.capacity),
            spawn_table_len: emitters.spawns.len(),
            frame: *frame,
            modifiers,
            gravity_enabled: gravity.is_some() as u32,
            gravity_strength: gravity.map_or(0.0, |g| g.strength),
            // Must stay above 0: a body's pull on itself is 0 · (0 + ε²)^(-3/2), NaN at ε = 0.
            gravity_softening_sq: gravity.map_or(1.0, |g| g.softening.max(1e-3).powi(2)),
            gravity_method: gravity.map_or(0, |g| g.method.as_gpu()),
            gravity_max_bodies: buffers.capacity.min(MAX_GRAVITY_BODIES),
            gravity_theta_sq: gravity.map_or(0.0, |g| g.theta * g.theta),
        };
        uploads.0.push(SpaceUpload {
            entity,
            buffers: buffers.clone(),
            params,
            render: render.cloned().unwrap_or_default(),
            z: position.z,
            visible: visibility.get(),
            layers: layers.cloned().unwrap_or_default(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emitter::{EmitterPlugin, ParticleEmitter};

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            TransformPlugin,
            crate::modifier::ModifierPlugin,
            SpacePlugin,
            EmitterPlugin,
        ));
        app.init_asset::<ShaderBuffer>().init_resource::<GpuSimReady>();
        app
    }

    fn space(capacity: u32) -> ParticleSpace {
        ParticleSpace { capacity }
    }

    fn uploads(app: &App) -> Vec<(u32, u32)> {
        let uploads = &app.world().resource::<SpaceUploads>().0;
        let mut list: Vec<_> =
            uploads.iter().map(|u| (u.params.capacity, u.params.spawn_total)).collect();
        list.sort();
        list
    }

    /// Pretends the GPU simulates every current space.
    fn simulate_all(app: &mut App) {
        let uploads = &app.world().resource::<SpaceUploads>().0;
        let ids = uploads.iter().map(|u| u.buffers.id()).collect();
        app.world().resource::<GpuSimReady>().set(ids);
    }

    #[test]
    fn spaces_are_uploaded_and_removed() {
        let mut app = app();
        let first = app.world_mut().spawn(space(1)).id();
        let second = app.world_mut().spawn(space(2)).id();
        app.world_mut().spawn(space(0)); // Ignored.
        app.update();
        assert_eq!(uploads(&app), [(1, 0), (2, 0)]);

        app.world_mut().despawn(second);
        app.update();
        assert_eq!(uploads(&app), [(1, 0)]);
        app.world_mut().entity_mut(first).remove::<ParticleSpace>();
        app.update();
        assert!(uploads(&app).is_empty());
        assert!(!app.world().entity(first).contains::<ParticleBuffers>());
    }

    #[test]
    fn emitters_spawn_into_their_nearest_space_once_it_is_simulated() {
        let mut app = app();
        let burst = |burst_count| ParticleEmitter { burst_count, rate: 0.0, ..default() };
        app.world_mut().spawn((space(10), children![burst(3)]));
        // Nested under a plain entity, and a space inside a space.
        app.world_mut().spawn((
            space(20),
            children![
                (Transform::default(), children![burst(4)]),
                (space(30), children![burst(5)]),
            ],
        ));
        app.world_mut().spawn(burst(100)); // No space.
        app.update();
        assert_eq!(uploads(&app), [(10, 0), (20, 0), (30, 0)]);

        simulate_all(&mut app);
        app.update();
        assert_eq!(uploads(&app), [(10, 3), (20, 4), (30, 5)]);
        app.update();
        assert_eq!(uploads(&app), [(10, 0), (20, 0), (30, 0)]);
    }

    #[test]
    fn recreated_buffers_wait_for_the_gpu_again() {
        let mut app = app();
        let entity = app.world_mut().spawn(space(10)).id();
        app.update();
        simulate_all(&mut app);
        app.world_mut().entity_mut(entity).remove::<ParticleSpace>();
        app.world_mut().entity_mut(entity).insert(ParticleSpace { capacity: 20 });
        app.update();
        assert_eq!(uploads(&app), [(20, 0)]);
        let id = app.world().get::<ParticleBuffers>(entity).unwrap().id();
        assert!(!app.world().resource::<GpuSimReady>().is_simulating(id));
    }

    #[test]
    fn capacity_is_limited_by_buffer_size_and_dispatch_size() {
        // WebGPU's defaults.
        assert_eq!(max_capacity(128 << 20, 65535), 2 << 20);
        assert_eq!(max_capacity(u64::MAX, 65535), 65535 * 256);
    }

    #[test]
    fn spawns_never_exceed_capacity() {
        let mut app = app();
        let burst = || ParticleEmitter { burst_count: u32::MAX, rate: 1e6, ..default() };
        app.world_mut().spawn((space(10), children![burst(), burst()]));
        app.update();
        simulate_all(&mut app);
        app.update();
        assert_eq!(uploads(&app), [(10, 10)]);
    }
}
