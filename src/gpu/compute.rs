use std::borrow::Cow;

use bevy::{
    core_pipeline::schedule::camera_driver,
    prelude::*,
    render::{
        render_asset::RenderAssets,
        render_resource::{
            binding_types::{storage_buffer_sized, uniform_buffer},
            *,
        },
        renderer::{RenderContext, RenderDevice, RenderGraph, RenderQueue},
        storage::GpuShaderBuffer,
        Render, RenderStartup, RenderSystems,
    },
};

use crate::{particle::ParticleBuffer, space::SimParams};

const SHADER_ASSET_PATH: &str = "shaders/particle_integrate.wgsl";
/// Must match `@workgroup_size` in the shader.
const WORKGROUP_SIZE: u32 = 256;

pub(super) fn build(render_app: &mut SubApp) {
    render_app
        .init_resource::<SimParamsBuffer>()
        .add_systems(RenderStartup, init_pipeline)
        .add_systems(
            Render,
            (
                write_sim_params.in_set(RenderSystems::PrepareResources),
                prepare_bind_group.in_set(RenderSystems::PrepareBindGroups),
            ),
        )
        // Simulate before any camera renders, so every view draws this frame's positions.
        .add_systems(RenderGraph, dispatch_integrate.before(camera_driver));
}

#[derive(Resource)]
struct IntegratePipeline {
    layout: BindGroupLayoutDescriptor,
    pipeline: CachedComputePipelineId,
}

#[derive(Resource, Default)]
struct SimParamsBuffer(UniformBuffer<SimParams>);

#[derive(Resource)]
struct IntegrateBindGroup(BindGroup);

fn init_pipeline(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    pipeline_cache: Res<PipelineCache>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "particle_integrate_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer::<SimParams>(false),
                storage_buffer_sized(false, None),
            ),
        ),
    );
    let pipeline = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("particle_integrate".into()),
        layout: vec![layout.clone()],
        shader: asset_server.load(SHADER_ASSET_PATH),
        entry_point: Some(Cow::from("integrate")),
        ..default()
    });
    commands.insert_resource(IntegratePipeline { layout, pipeline });
}

fn write_sim_params(
    params: Res<SimParams>,
    mut buffer: ResMut<SimParamsBuffer>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    buffer.0.set(*params);
    buffer.0.write_buffer(&device, &queue);
}

fn prepare_bind_group(
    mut commands: Commands,
    pipeline: Res<IntegratePipeline>,
    pipeline_cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    params: Res<SimParamsBuffer>,
    particle_buffer: Option<Res<ParticleBuffer>>,
    gpu_buffers: Res<RenderAssets<GpuShaderBuffer>>,
) {
    let Some(particles) = particle_buffer.and_then(|b| gpu_buffers.get(&b.buffer)) else {
        commands.remove_resource::<IntegrateBindGroup>();
        return;
    };
    let bind_group = device.create_bind_group(
        "particle_integrate_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipeline.layout),
        &BindGroupEntries::sequential((&params.0, particles.buffer.as_entire_binding())),
    );
    commands.insert_resource(IntegrateBindGroup(bind_group));
}

fn dispatch_integrate(
    mut ctx: RenderContext,
    pipeline: Res<IntegratePipeline>,
    pipeline_cache: Res<PipelineCache>,
    bind_group: Option<Res<IntegrateBindGroup>>,
    params: Res<SimParams>,
) {
    let (Some(bind_group), Some(compute_pipeline)) = (
        bind_group,
        pipeline_cache.get_compute_pipeline(pipeline.pipeline),
    ) else {
        return;
    };
    if params.count == 0 {
        return;
    }

    let mut pass = ctx
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("particle_integrate"),
            ..default()
        });
    pass.set_pipeline(compute_pipeline);
    pass.set_bind_group(0, &bind_group.0, &[]);
    pass.dispatch_workgroups(params.count.div_ceil(WORKGROUP_SIZE), 1, 1);
}
