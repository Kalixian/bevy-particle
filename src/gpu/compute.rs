use std::borrow::Cow;

use bevy::{
    core_pipeline::schedule::camera_driver,
    prelude::*,
    render::{
        render_asset::RenderAssets,
        render_resource::{
            binding_types::{storage_buffer_read_only, storage_buffer_sized, uniform_buffer},
            *,
        },
        renderer::{RenderContext, RenderDevice, RenderGraph, RenderQueue},
        storage::GpuShaderBuffer,
        Render, RenderStartup, RenderSystems,
    },
};

use super::GpuSimReady;
use crate::{
    emitter::{EmitterUpload, GpuEmitter},
    modifier::{GpuModifier, ModifierUpload},
    particle::ParticleBuffers,
    space::SimParams,
};

const SHADER_ASSET_PATH: &str = "shaders/particle_sim.wgsl";
/// Must match `@workgroup_size` in the shader.
const WORKGROUP_SIZE: u32 = 256;

pub(super) fn build(render_app: &mut SubApp) {
    render_app
        .init_resource::<SimBuffers>()
        .add_systems(RenderStartup, init_pipelines)
        .add_systems(
            Render,
            (
                write_sim_buffers.in_set(RenderSystems::PrepareResources),
                prepare_bind_group.in_set(RenderSystems::PrepareBindGroups),
            ),
        )
        // Simulate before any camera renders, so every view draws this frame's state.
        .add_systems(RenderGraph, dispatch_sim.before(camera_driver));
}

#[derive(Resource)]
struct SimPipelines {
    layout: BindGroupLayoutDescriptor,
    integrate: CachedComputePipelineId,
    spawn: CachedComputePipelineId,
}

/// Small per-frame buffers written from the CPU.
#[derive(Resource, Default)]
struct SimBuffers {
    params: UniformBuffer<SimParams>,
    emitters: StorageBuffer<Vec<GpuEmitter>>,
    modifiers: StorageBuffer<Vec<GpuModifier>>,
}

#[derive(Resource)]
struct SimBindGroup(BindGroup);

fn init_pipelines(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    pipeline_cache: Res<PipelineCache>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "particle_sim_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer::<SimParams>(false),
                // particles
                storage_buffer_sized(false, None),
                // free list
                storage_buffer_sized(false, None),
                storage_buffer_read_only::<Vec<GpuEmitter>>(false),
                storage_buffer_read_only::<Vec<GpuModifier>>(false),
            ),
        ),
    );
    let shader: Handle<Shader> = asset_server.load(SHADER_ASSET_PATH);
    let queue = |entry_point: &'static str| {
        pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(format!("particle_{entry_point}").into()),
            layout: vec![layout.clone()],
            shader: shader.clone(),
            entry_point: Some(Cow::from(entry_point)),
            ..default()
        })
    };
    let integrate = queue("integrate");
    let spawn = queue("spawn");
    commands.insert_resource(SimPipelines {
        layout,
        integrate,
        spawn,
    });
}

fn write_sim_buffers(
    params: Res<SimParams>,
    emitters: Res<EmitterUpload>,
    modifiers: Res<ModifierUpload>,
    mut buffers: ResMut<SimBuffers>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    buffers.params.set(*params);
    buffers.params.write_buffer(&device, &queue);
    buffers.emitters.set(emitters.emitters.clone());
    buffers.emitters.write_buffer(&device, &queue);
    buffers.modifiers.set(modifiers.gpu_data());
    buffers.modifiers.write_buffer(&device, &queue);
}

fn prepare_bind_group(
    mut commands: Commands,
    pipelines: Res<SimPipelines>,
    pipeline_cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    sim_buffers: Res<SimBuffers>,
    particle_buffers: Option<Res<ParticleBuffers>>,
    gpu_buffers: Res<RenderAssets<GpuShaderBuffer>>,
) {
    let gpu = particle_buffers.and_then(|b| {
        Some((gpu_buffers.get(&b.particles)?, gpu_buffers.get(&b.free_list)?))
    });
    let (Some((particles, free_list)), Some(emitters), Some(modifiers)) = (
        gpu,
        sim_buffers.emitters.binding(),
        sim_buffers.modifiers.binding(),
    ) else {
        commands.remove_resource::<SimBindGroup>();
        return;
    };
    let bind_group = device.create_bind_group(
        "particle_sim_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipelines.layout),
        &BindGroupEntries::sequential((
            &sim_buffers.params,
            particles.buffer.as_entire_binding(),
            free_list.buffer.as_entire_binding(),
            emitters,
            modifiers,
        )),
    );
    commands.insert_resource(SimBindGroup(bind_group));
}

fn dispatch_sim(
    mut ctx: RenderContext,
    pipelines: Res<SimPipelines>,
    pipeline_cache: Res<PipelineCache>,
    bind_group: Option<Res<SimBindGroup>>,
    params: Res<SimParams>,
    ready: Res<GpuSimReady>,
) {
    let (Some(bind_group), Some(integrate), Some(spawn)) = (
        bind_group,
        pipeline_cache.get_compute_pipeline(pipelines.integrate),
        pipeline_cache.get_compute_pipeline(pipelines.spawn),
    ) else {
        return;
    };
    if params.capacity == 0 {
        return;
    }

    let mut pass = ctx
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("particle_sim"),
            ..default()
        });
    pass.set_bind_group(0, &bind_group.0, &[]);

    // Integrate first: it ages and kills particles, returning their slots to the free list,
    // so the spawn pass can reuse them in the same frame. Each dispatch is its own
    // synchronization scope, so spawn sees integrate's writes.
    pass.set_pipeline(integrate);
    pass.dispatch_workgroups(params.capacity.div_ceil(WORKGROUP_SIZE), 1, 1);

    if params.spawn_total > 0 {
        pass.set_pipeline(spawn);
        pass.dispatch_workgroups(params.spawn_total.div_ceil(WORKGROUP_SIZE), 1, 1);
    }

    ready.set();
}
