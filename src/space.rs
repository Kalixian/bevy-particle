use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::{extract_resource::ExtractResource, render_resource::ShaderType, storage::ShaderBuffer},
};

use crate::{
    emitter::{EmitterSystems, EmitterUpload},
    modifier::{ModifierSystems, ModifierUpload, Modifiers},
    particle::{GpuParticle, ParticleBuffers},
};

/// The outer layer: settings and forces that apply to every particle in the space.
/// Space-wide behavior comes from a [`Modifiers`] component on the space entity.
pub struct SpacePlugin;

impl Plugin for SpacePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SimParams>().add_systems(
            PostUpdate,
            (
                create_space_buffers,
                update_sim_params
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
#[derive(Component, Debug, Clone)]
#[require(Transform)]
pub struct ParticleSpace {
    /// Maximum number of live particles. Fixed once the space is created.
    pub capacity: u32,
}

/// Per-frame simulation uniforms. Must match `struct SimParams` in `particle_sim.wgsl`.
#[derive(Resource, Clone, Copy, Default, ExtractResource, ShaderType)]
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
}

fn create_space_buffers(
    mut commands: Commands,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    spaces: Query<(Entity, &ParticleSpace), Added<ParticleSpace>>,
    existing: Option<Res<ParticleBuffers>>,
) {
    for (entity, space) in &spaces {
        if existing.is_some() {
            warn!("Only one ParticleSpace is supported; ignoring {entity}");
            continue;
        }
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

        commands.insert_resource(ParticleBuffers {
            particles: buffers.add(particles),
            free_list: buffers.add(free_list),
            capacity,
        });
        return;
    }
}

fn update_sim_params(
    time: Res<Time>,
    particle_buffers: Option<Res<ParticleBuffers>>,
    emitters: Res<EmitterUpload>,
    mut modifier_upload: ResMut<ModifierUpload>,
    space: Query<(&GlobalTransform, Option<&Modifiers>), With<ParticleSpace>>,
    mut params: ResMut<SimParams>,
) {
    let (Some((transform, modifiers)), Some(buffers)) = (space.iter().next(), particle_buffers)
    else {
        *params = SimParams::default();
        return;
    };
    let (modifier_offset, modifier_count) =
        modifier_upload.push(modifiers, transform.translation().truncate());

    *params = SimParams {
        dt: time.delta_secs(),
        capacity: buffers.capacity,
        spawn_total: emitters.spawn_total,
        emitter_count: emitters.emitters.len() as u32,
        frame: params.frame.wrapping_add(1),
        modifier_offset,
        modifier_count,
    };
}
